// Derived from RaoFoundation/subtensor sdk/python/bittensor/extension/browser.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Browser selection and detached launch shared by Vault and the extension.

#[cfg(unix)]
use std::path::Path;
#[cfg(any(unix, test))]
use std::path::PathBuf;

#[cfg(any(unix, test))]
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct BrowserCommand {
    pub(crate) program: PathBuf,
    pub(crate) arguments: Vec<String>,
}

#[cfg(any(unix, test))]
pub(crate) fn browser_command_with(
    url: &str,
    browser: Option<&str>,
    environment: impl FnOnce() -> Option<String>,
    macos: bool,
    mut which: impl FnMut(&str) -> Option<PathBuf>,
) -> BrowserCommand {
    let system = || BrowserCommand {
        program: if macos { "/usr/bin/open" } else { "xdg-open" }.into(),
        arguments: vec![url.into()],
    };
    // Python uses `explicit or getenv`: an empty explicit choice falls through
    // to the environment, while a custom command retains its original bytes.
    let choice = browser
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(environment);
    let Some(choice) = choice.filter(|value| !value.is_empty()) else {
        return system();
    };
    let normalized = choice
        .trim_matches(|character: char| {
            character.is_whitespace() || matches!(character, '\u{1c}'..='\u{1f}')
        })
        .to_lowercase();
    if matches!(normalized.as_str(), "default" | "system") {
        return system();
    }
    if macos {
        let app = match normalized.as_str() {
            "firefox" | "ff" => "Firefox",
            "chrome" | "google-chrome" => "Google Chrome",
            "chromium" => "Chromium",
            "brave" => "Brave Browser",
            "edge" => "Microsoft Edge",
            "safari" => "Safari",
            _ => &choice,
        };
        return BrowserCommand {
            program: "/usr/bin/open".into(),
            arguments: vec!["-a".into(), app.into(), url.into()],
        };
    }
    let candidates: &[&str] = match normalized.as_str() {
        "firefox" | "ff" => &["firefox", "firefox-esr"],
        "chrome" | "google-chrome" => &[
            "google-chrome",
            "google-chrome-stable",
            "chromium",
            "chromium-browser",
        ],
        "brave" => &["brave-browser", "brave"],
        "edge" => &["microsoft-edge", "microsoft-edge-stable"],
        _ => &[&choice],
    };
    for candidate in candidates {
        if let Some(program) = which(candidate) {
            return BrowserCommand {
                program,
                arguments: vec![url.into()],
            };
        }
    }
    system()
}

#[cfg(unix)]
fn browser_executable(name: &str) -> Option<PathBuf> {
    use nix::unistd::{AccessFlags, access};
    let executable = |path: &Path| path.is_file() && access(path, AccessFlags::X_OK).is_ok();
    if name.contains('/') {
        let path = Path::new(name);
        return executable(path).then(|| path.to_owned());
    }
    let path = std::env::var_os("PATH").unwrap_or_else(|| "/bin:/usr/bin".into());
    if path.is_empty() {
        return None;
    }
    std::env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find(|path| executable(path))
}

/// A browser launch failure without provider arguments or session URL data.
#[derive(Debug)]
pub(crate) enum OpenError {
    #[cfg(not(unix))]
    Unsupported,
    #[cfg(unix)]
    Selection,
    #[cfg(unix)]
    Spawn,
}

/// Select and spawn the browser without owning its application lifetime.
pub(crate) async fn open_page(url: &str, browser: Option<&str>) -> Result<(), OpenError> {
    #[cfg(not(unix))]
    {
        let _ = (url, browser);
        Err(OpenError::Unsupported)
    }
    #[cfg(unix)]
    {
        use std::process::Stdio;
        use tokio::process::Command;

        let url = url.to_owned();
        let browser = browser.map(str::to_owned);
        let selected = tokio::task::spawn_blocking(move || {
            browser_command_with(
                &url,
                browser.as_deref(),
                || std::env::var("BT_EXTENSION_BROWSER").ok(),
                cfg!(target_os = "macos"),
                browser_executable,
            )
        })
        .await
        .map_err(|_| OpenError::Selection)?;
        let mut command = Command::new(selected.program);
        command
            .args(selected.arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        drop(spawn_detached(command).map_err(|_| OpenError::Spawn)?);
        Ok(())
    }
}

#[cfg(unix)]
fn spawn_detached(mut command: tokio::process::Command) -> std::io::Result<tokio::process::Child> {
    use std::os::unix::process::CommandExt;

    // A browser is a user application, not a short-lived session subprocess.
    // A separate process group avoids forwarding the caller's terminal signals;
    // dropping a launcher handle must not kill the browser.
    command.as_std_mut().process_group(0);
    command.kill_on_drop(false).spawn()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // A controlled pipe child proves launch returns while the application is
    // waiting, and that dropping the handle neither kills it nor shares our
    // terminal process group. No browser, display, shell expansion of inputs,
    // filesystem fixture or background service is needed.
    #[tokio::test]
    async fn detached_child_survives_launcher_handle_drop_and_exits_after_pipe_reply()
    -> Result<(), Box<dyn std::error::Error>> {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            use nix::unistd::{Pid, getpgid};
            use std::process::Stdio;

            let mut command = tokio::process::Command::new("/bin/sh");
            command
                .arg("-c")
                .arg("printf ready; if IFS= read -r marker; then printf survived; fi")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            let mut child = spawn_detached(command)?;
            let pid = child.id().ok_or("child has no PID")?;
            assert_eq!(
                getpgid(Some(Pid::from_raw(i32::try_from(pid)?)))?.as_raw(),
                i32::try_from(pid)?
            );
            let mut stdin = child.stdin.take().ok_or("child stdin missing")?;
            let mut stdout = child.stdout.take().ok_or("child stdout missing")?;
            let mut ready = [0; 5];
            stdout.read_exact(&mut ready).await?;
            assert_eq!(&ready, b"ready");
            drop(child);
            stdin.write_all(b"finish\n").await?;
            drop(stdin);
            let mut survived = [0; 8];
            stdout.read_exact(&mut survived).await?;
            assert_eq!(&survived, b"survived");
            let mut end = [0; 1];
            assert_eq!(stdout.read(&mut end).await?, 0);
            Ok::<_, Box<dyn std::error::Error>>(())
        })
        .await??;
        Ok(())
    }
}
