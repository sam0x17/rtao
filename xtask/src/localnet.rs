//! Rust-only localnet provisioning and once-per-container Alice bootstrap.

use std::{
    fs,
    io::Write,
    path::{Component, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use rtao_testkit::{Localnet, LocalnetProfile};
use serde_json::json;

#[derive(Debug, PartialEq, Eq)]
struct Options {
    command: String,
    endpoint: Option<String>,
    lane: String,
    profile: LocalnetProfile,
    output: Option<PathBuf>,
}
fn parse(args: Vec<String>) -> Result<Options> {
    let mut args = args.into_iter();
    let command = args.next().context("usage: localnet bootstrap|up [--endpoint ws://127.0.0.1:PORT] [--lane pinned] [--profile fast|slow] [--out json]")?;
    ensure!(
        ["bootstrap", "up"].contains(&command.as_str()),
        "localnet command must be bootstrap or up"
    );
    let mut options = Options {
        command,
        endpoint: None,
        lane: "pinned".into(),
        profile: LocalnetProfile::Fast,
        output: None,
    };
    while let Some(argument) = args.next() {
        let value = args
            .next()
            .with_context(|| format!("{argument} requires a value"))?;
        match argument.as_str() {
            "--endpoint" => {
                ensure!(options.endpoint.is_none(), "duplicate --endpoint");
                options.endpoint = Some(value);
            }
            "--lane" => {
                ensure!(
                    ["pinned", "mainnet", "testnet", "devnet", "main"].contains(&value.as_str()),
                    "unknown localnet lane"
                );
                options.lane = value;
            }
            "--profile" => options.profile = LocalnetProfile::parse(&value)?,
            "--out" => {
                ensure!(options.output.is_none(), "duplicate --out");
                options.output = Some(value.into());
            }
            _ => bail!("unknown localnet option {argument}"),
        }
    }
    if options.command == "bootstrap" {
        ensure!(options.endpoint.is_some(), "bootstrap requires --endpoint");
        ensure!(options.output.is_none(), "bootstrap does not accept --out");
    } else {
        ensure!(options.endpoint.is_none(), "up does not accept --endpoint");
        ensure!(
            options.output.is_some(),
            "up requires --out to record the owned container and endpoint for cleanup"
        );
    }
    Ok(options)
}

/// Execute localnet setup without invoking Python or writing a wallet keyfile.
pub(crate) fn run(args: Vec<String>) -> Result<()> {
    if args == ["contracts", "--check"] {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace root missing")?;
        check_contracts(&fs::read_to_string(
            root.join("crates/rtao-testkit/tests/e2e.rs"),
        )?)?;
        println!("all ten M2 scenario contracts exist and are enabled");
        return Ok(());
    }
    let options = parse(args)?;
    let task_root = PathBuf::from(
        std::env::var_os("RTAO_TEMP_ROOT")
            .context("set RTAO_TEMP_ROOT to the recorded task-scoped root")?,
    );
    ensure!(task_root.is_absolute(), "RTAO_TEMP_ROOT must be absolute");
    fs::create_dir_all(task_root.join("e2e"))?;
    if let Some(output) = &options.output {
        ensure!(
            output.is_absolute() && output.starts_with(&task_root),
            "localnet --out must be an absolute path beneath RTAO_TEMP_ROOT"
        );
        ensure!(
            !output
                .components()
                .any(|component| matches!(component, Component::ParentDir)),
            "localnet --out cannot contain parent traversal"
        );
        ensure!(
            !output.exists(),
            "localnet --out already exists; refusing to overwrite container cleanup identity"
        );
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let result: Result<()> = runtime.block_on(async {
        if let Some(endpoint) = options.endpoint {
            let localnet = Localnet::connect(&endpoint, options.profile).await?;
            localnet.bootstrap().await?;
            println!("localnet bootstrap complete: {} ({})", localnet.endpoint(), localnet.profile().name());
            localnet.client().close().await;
        } else {
            let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().context("workspace root missing")?;
            let image = Localnet::pinned_image(&fs::read_to_string(root.join("upstream.toml"))?, &options.lane)?;
            let mut localnet = Localnet::start_image(&image, options.profile, std::env::var_os("SKIP_PULL").is_some()).await?;
            localnet.bootstrap().await?;
            let output = options.output.context("output missing")?;
            let report = json!({"schema":1, "container_id":localnet.container_id().context("owned localnet missing container ID")?, "endpoint":localnet.endpoint(), "profile":localnet.profile().name(), "lane":options.lane, "image":image, "bootstrap":true});
            if let Some(parent) = output.parent() {
                fs::create_dir_all(parent)?;
                ensure!(parent.canonicalize()?.starts_with(task_root.canonicalize()?), "localnet --out parent resolves outside RTAO_TEMP_ROOT");
            }
            let mut report_file = fs::OpenOptions::new().write(true).create_new(true).open(&output)?;
            writeln!(report_file, "{}", serde_json::to_string_pretty(&report)?)?;
            report_file.sync_all()?;
            localnet.client().close().await;
            let id = localnet.detach_container().context("container lost before ownership transfer")?;
            println!("localnet ready: {}; cleanup docker container {} after tests", output.display(), id);
        }
        Ok(())
    });
    if let Err(error) = &result
        && matches!(
            error.downcast_ref::<rtao_testkit::LocalnetError>(),
            Some(rtao_testkit::LocalnetError::DrandUnreachable)
        )
        && std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true")
    {
        eprintln!(
            "::error title=DrandUnreachable::No quicknet pulse after one fresh-container retry; verify localnet outbound HTTPS"
        );
    }
    result
}

const REQUIRED_SCENARIOS: &[&str] = &[
    "scenario_01_",
    "scenario_02_",
    "scenario_03_",
    "scenario_05_",
    "scenario_06_",
    "scenario_07_",
    "scenario_08_",
    "scenario_15_",
    "scenario_16_",
    "scenario_17_",
    "scenario_18_",
];
fn check_contracts(source: &str) -> Result<()> {
    let source = syn::parse_file(source)?;
    let functions: Vec<_> = source
        .items
        .iter()
        .filter_map(|item| {
            if let syn::Item::Fn(function) = item {
                Some(function)
            } else {
                None
            }
        })
        .collect();
    for prefix in REQUIRED_SCENARIOS {
        let matched: Vec<_> = functions
            .iter()
            .filter(|function| function.sig.ident.to_string().starts_with(prefix))
            .collect();
        ensure!(
            !matched.is_empty(),
            "M2 required scenario {prefix} has no test"
        );
        for function in matched {
            let name = function.sig.ident.to_string();
            ensure!(
                function.attrs.iter().any(|attr| attr
                    .path()
                    .segments
                    .last()
                    .is_some_and(|segment| segment.ident == "test")),
                "{name} is not a test"
            );
            ensure!(
                !function
                    .attrs
                    .iter()
                    .any(|attr| attr.path().is_ident("ignore")
                        || attr.path().is_ident("cfg")
                        || attr.path().is_ident("cfg_attr")),
                "M2 required scenario {name} is ignored or conditionally omitted; its production dependency is unfinished"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn localnet_arguments_require_explicit_bootstrap_and_cleanup_identity() -> Result<()> {
        assert_eq!(
            parse(
                [
                    "bootstrap",
                    "--endpoint",
                    "ws://127.0.0.1:9944",
                    "--profile",
                    "slow"
                ]
                .map(str::to_owned)
                .to_vec()
            )?
            .profile,
            LocalnetProfile::Slow
        );
        for args in [
            vec![],
            vec!["bootstrap"],
            vec!["up"],
            vec!["up", "--out", "x", "--endpoint", "ws://127.0.0.1:9944"],
            vec!["bootstrap", "--endpoint", "a", "--endpoint", "b"],
        ] {
            assert!(parse(args.into_iter().map(str::to_owned).collect()).is_err());
        }
        Ok(())
    }
    #[test]
    fn required_scenarios_reject_absence_ignore_and_conditional_omission() -> Result<()> {
        let source = REQUIRED_SCENARIOS
            .iter()
            .map(|prefix| format!("#[tokio::test] async fn {prefix}case() {{}}\n"))
            .collect::<String>();
        check_contracts(&source)?;
        assert!(check_contracts("").is_err());
        for attribute in [
            "#[ignore]",
            "#[cfg(feature = \"later\")]",
            "#[cfg_attr(feature = \"later\", ignore)]",
        ] {
            assert!(check_contracts(&format!("{attribute}\n{source}")).is_err());
        }
        Ok(())
    }
}
