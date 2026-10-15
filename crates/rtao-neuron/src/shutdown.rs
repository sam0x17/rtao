//! First-signal cancellation and immediate second-signal exit.

use tokio_util::sync::CancellationToken;

pub(crate) fn spawn_signals(cancel: CancellationToken) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            let Ok(mut interrupt) = signal(SignalKind::interrupt()) else {
                cancel.cancel();
                return;
            };
            let Ok(mut terminate) = signal(SignalKind::terminate()) else {
                cancel.cancel();
                return;
            };
            tokio::select! { _ = interrupt.recv() => {}, _ = terminate.recv() => {} }
            cancel.cancel();
            tokio::select! { _ = interrupt.recv() => {}, _ = terminate.recv() => {} }
            std::process::exit(130);
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
            cancel.cancel();
            let _ = tokio::signal::ctrl_c().await;
            std::process::exit(130);
        }
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        path::PathBuf,
        process::{Command, Stdio},
        time::Duration,
    };
    #[tokio::test]
    async fn signal_child() -> anyhow::Result<()> {
        let Some(root) = std::env::var_os("RTAO_SHUTDOWN_CHILD_ROOT") else {
            return Ok(());
        };
        let root = PathBuf::from(root);
        let cancel = CancellationToken::new();
        let signals = spawn_signals(cancel.clone());
        // Give both handlers a scheduling opportunity before the parent signals.
        tokio::time::sleep(Duration::from_millis(20)).await;
        std::fs::write(root.join("ready"), b"ready")?;
        cancel.cancelled().await;
        std::fs::write(root.join("cancelled"), b"cancelled")?;
        signals.await?;
        Ok(())
    }
    #[tokio::test]
    async fn first_signal_cancels_and_second_exits_130() -> anyhow::Result<()> {
        let recorded = std::env::var_os("RTAO_TEMP_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("rtao-neuron-tests"));
        let root = recorded
            .join("tmp")
            .join(format!("neuron-signals-{}", std::process::id()));
        std::fs::create_dir_all(&root)?;
        let mut child = Command::new(std::env::current_exe()?)
            .args(["--exact", "shutdown::tests::signal_child", "--nocapture"])
            .env("RTAO_SHUTDOWN_CHILD_ROOT", &root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        async fn wait(path: PathBuf) -> anyhow::Result<()> {
            tokio::time::timeout(Duration::from_secs(10), async {
                while !path.exists() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await?;
            Ok(())
        }
        let outcome = async {
            wait(root.join("ready")).await?;
            anyhow::ensure!(
                Command::new("kill")
                    .args(["-TERM", &child.id().to_string()])
                    .status()?
                    .success(),
                "could not signal own child"
            );
            wait(root.join("cancelled")).await?;
            anyhow::ensure!(
                child.try_wait()?.is_none(),
                "first signal exited before graceful shutdown"
            );
            anyhow::ensure!(
                Command::new("kill")
                    .args(["-INT", &child.id().to_string()])
                    .status()?
                    .success(),
                "could not signal own child"
            );
            let status = child.wait()?;
            anyhow::ensure!(
                status.code() == Some(130),
                "second signal returned {status}"
            );
            Ok::<_, anyhow::Error>(())
        }
        .await;
        if child.try_wait()?.is_none() {
            let _ = child.kill();
            let _ = child.wait();
        }
        for name in ["ready", "cancelled"] {
            let path = root.join(name);
            if path.exists() {
                std::fs::remove_file(path)?;
            }
        }
        std::fs::remove_dir(root)?;
        outcome
    }
}
