//! Verify the built validator image against a real, isolated TLS FakeNode.

use std::{error::Error, fs, path::PathBuf, time::Duration};

use base64::{Engine, engine::general_purpose::STANDARD};
use rtao_testkit::FakeNode;
use tokio::process::Command;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    if std::env::consts::OS != "linux" {
        return Err("validator image TLS verification requires Linux host networking".into());
    }
    let image = std::env::var("RTAO_VALIDATOR_IMAGE")?;
    if !image.starts_with("sha256:")
        || image.len() != 71
        || !image[7..].bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("RTAO_VALIDATOR_IMAGE must be the exact built Docker image ID".into());
    }
    let temporary = PathBuf::from(std::env::var("RTAO_TEMP_ROOT")?).canonicalize()?;
    let directory = temporary.join(format!("validator-tls-{}", std::process::id()));
    fs::create_dir(&directory)?;
    let result = verify(&image, &directory).await;
    if directory.join("container-name").exists() {
        return Err(format!(
            "container cleanup could not be verified; retained its certificate mount at {}: {result:?}",
            directory.display()
        ).into());
    }
    // These two paths were created by this process and are never wallet paths.
    if directory.join("root.pem").is_file() {
        fs::remove_file(directory.join("root.pem"))?;
    }
    fs::remove_dir(&directory)?;
    result
}

async fn verify(image: &str, directory: &std::path::Path) -> Result<(), Box<dyn Error>> {
    let mut node = FakeNode::start_tls().await?;
    let certificate = node.extra_root().ok_or("TLS FakeNode has no certificate")?;
    let encoded = STANDARD.encode(certificate.as_ref());
    let mut pem = String::from("-----BEGIN CERTIFICATE-----\n");
    for chunk in encoded.as_bytes().chunks(64) {
        pem.push_str(std::str::from_utf8(chunk)?);
        pem.push('\n');
    }
    pem.push_str("-----END CERTIFICATE-----\n");
    fs::write(directory.join("root.pem"), pem)?;

    let untrusted = run(image, node.url(), directory, false).await?;
    let untrusted_stderr = String::from_utf8_lossy(&untrusted.stderr);
    if untrusted.status.success() || !untrusted_stderr.to_lowercase().contains("certificate") {
        return Err(format!(
            "untrusted certificate did not cause a TLS refusal: {untrusted_stderr}"
        )
        .into());
    }
    if !node.requests().is_empty() {
        return Err("untrusted connection reached the RPC application".into());
    }

    let trusted = run(image, node.url(), directory, true).await?;
    if !trusted.status.success() {
        return Err(format!(
            "trusted validator connection failed: {}",
            String::from_utf8_lossy(&trusted.stderr)
        )
        .into());
    }
    let requests = node.requests();
    for required in ["chain_getBlockHash", "state_getRuntimeVersion"] {
        if !requests.iter().any(|request| request.method == required) {
            return Err(format!("validator startup omitted {required}").into());
        }
    }
    if !requests.iter().any(|request| {
        request.method == "state_call"
            && request.params[0].as_str() == Some("Metadata_metadata_at_version")
            && request.params[1].as_str() == Some("0x0f000000")
    }) {
        return Err("validator startup omitted the V15 runtime metadata request".into());
    }
    println!(
        "Validator image connected over wss://, verified genesis/runtime, and rejected an untrusted certificate."
    );
    node.shutdown().await;
    Ok(())
}

async fn run(
    image: &str,
    endpoint: &str,
    directory: &std::path::Path,
    trusted: bool,
) -> Result<std::process::Output, Box<dyn Error>> {
    let name = format!(
        "rtao-validator-tls-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    );
    fs::write(directory.join("container-name"), &name)?;
    let mut command = Command::new("docker");
    command.args([
        "run",
        "--rm",
        "--network",
        "host",
        "--read-only",
        "--cap-drop",
        "ALL",
        "--security-opt",
        "no-new-privileges",
        "--tmpfs",
        "/tmp:rw,nosuid,nodev,size=16m",
        "--name",
        &name,
        "--label",
    ]);
    command.arg(format!("io.rtao.validator-tls={name}"));
    if trusted {
        command.args(["--env", "SSL_CERT_FILE=/certificates/root.pem", "--mount"]);
        command.arg(format!(
            "type=bind,src={},dst=/certificates,readonly",
            directory.display()
        ));
    }
    command.args([
        image,
        "--check-connection",
        "--network",
        "finney",
        "--chain-endpoint",
        endpoint,
    ]);
    command.kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(60), command.output()).await;
    cleanup(&name).await?;
    fs::remove_file(directory.join("container-name"))?;
    Ok(output??)
}

async fn cleanup(name: &str) -> Result<(), Box<dyn Error>> {
    let inspected = tokio::time::timeout(
        Duration::from_secs(15),
        Command::new("docker")
            .args([
                "inspect",
                "--type",
                "container",
                "--format",
                "{{index .Config.Labels \"io.rtao.validator-tls\"}}",
                name,
            ])
            .kill_on_drop(true)
            .output(),
    )
    .await??;
    if !inspected.status.success() {
        let error = String::from_utf8_lossy(&inspected.stderr);
        if error.contains("No such object") || error.contains("No such container") {
            return Ok(());
        }
        return Err(format!("cannot verify owned container cleanup: {error}").into());
    }
    if String::from_utf8(inspected.stdout)?.trim() != name {
        return Err("refusing to remove a container with a different ownership label".into());
    }
    let removed = tokio::time::timeout(
        Duration::from_secs(15),
        Command::new("docker")
            .args(["rm", "--force", name])
            .kill_on_drop(true)
            .output(),
    )
    .await??;
    if !removed.status.success() {
        return Err(format!(
            "owned container removal failed: {}",
            String::from_utf8_lossy(&removed.stderr)
        )
        .into());
    }
    Ok(())
}
