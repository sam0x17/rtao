//! Repository maintenance commands for rtao.

mod bench;
mod compat;
mod core_canary;
mod core_patch;
mod core_surface;
mod deps_check;
mod docs;
mod drift;
mod extract;
mod extract_receipt;
mod fingerprint;
mod fuzz;
mod gates;
mod import_upstream;
mod lint_upstream;
mod localnet;
mod metadata_pin;
mod network_pin;
mod oracle;
mod parity;
mod pin;
mod pin_pipeline;
mod pr_u4_proof;
mod probe;
mod read_hit_replacements;
mod record;
mod regen;
mod release;
mod release_notes;
mod staged;
mod supply;
mod template;
mod upstream_next;

use anyhow::{Result, bail};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("pin") => pin::run(args.collect()),
        Some("regen") => regen::run(args.collect()),
        Some("compat") => compat::run(args.collect()),
        Some("drift") => drift::run(args.collect()),
        Some("probe") => probe::run(args.collect()),
        Some("bench") => bench::run(args.collect()),
        Some("fuzz") => fuzz::run(args.collect()),
        Some("oracle") => oracle::run(args.collect()),
        Some("record") => record::run(args.collect()),
        Some("localnet") => localnet::run(args.collect()),
        Some("pr-u4-proof") => pr_u4_proof::entry(args.collect()),
        Some("upstream-next") => upstream_next::run(args.collect()),
        Some("docs") => docs::run(args.collect()),
        Some("release") => release::run(args.collect()),
        Some("release-notes") => release_notes::run(args.collect()),
        Some("gates") => gates::run(args.collect()),
        Some("supply") => supply::run(args.collect()),
        Some("template") => template::run(args.collect()),
        Some("core-surface") => core_surface::run(args.collect()),
        Some("core-patch") => core_patch::run(args.collect()),
        Some("lint-upstream") => lint_upstream::run(args.collect()),
        Some("staged") => staged::run(args.collect()),
        Some("extract") => extract::run(&args.collect::<Vec<_>>()),
        Some("deps-check") => deps_check::run(args.collect()),
        Some("import-upstream") => import_upstream::run(&args.collect::<Vec<_>>()),
        Some("parity") => parity::run(&args.collect::<Vec<_>>()),
        Some("help" | "--help" | "-h") | None => {
            println!(
                "Repository maintenance\n\n\
  cargo xtask gates [--job <name>] [--list]\n\
  cargo xtask supply\n\
  cargo xtask template check [--localnet]\n\
  cargo xtask pin --check\n\
  cargo xtask pin --check --prepare-cache (hosted source-cache bootstrap)\n\
  cargo xtask pin --networks [--check]\n\
  cargo xtask pin --parity <vN|auto> --check [--github-output <path>]\n\
  cargo xtask pin --parity <vN|auto>\n\
  cargo xtask pin --core-branch main --no-commit (disposable checkouts only)\n\
  cargo xtask import-upstream [--verify-only]\n\
  cargo xtask deps-check [--profile no-host]\n\
  cargo xtask extract [--check]\n\
  cargo xtask regen [--check] [--models-only|--namespaces-only]\n\
  cargo xtask compat [--check] [--milestone M<n>]\n\
  cargo xtask drift diff <old.v15.scale> <new.v15.scale> [--out <directory>]\n\
  cargo xtask drift --network <finney|testnet|devnet|localnet-main> [--check]\n\
  cargo xtask probe --networks finney,testnet [--output <json>] [--report <markdown>]\n\
  cargo xtask probe pins [--check]\n\
  cargo xtask bench [--compare-baseline] [--fail-over-percent 30]\n\
  cargo xtask fuzz [--toolchain nightly] [--seconds-per-target 120] [--list]\n\
  cargo xtask oracle inputs|image|dump|--check [--plan] [--push] [--image <oracle@sha256>] [--out directory] [--scope M2|all]\n\
  cargo xtask record --image <oracle@sha256> --localnet <owned-container-id> [--out directory] [--plan]\n\
  cargo xtask record --check [--out directory]\n\
  cargo xtask localnet bootstrap --endpoint ws://127.0.0.1:PORT [--profile fast|slow]\n\
  cargo xtask localnet up --lane pinned|mainnet|testnet|devnet|main --profile fast|slow --out <json>\n\
  cargo xtask localnet contracts --check\n\
  cargo xtask pr-u4-proof prepare\n\
  cargo xtask upstream-next [--branch main|testnet] [--out <json>]\n\
  cargo xtask docs --out <directory> [--llms] [--check]\n\
  cargo xtask release verify|local-record --tag rtao-vX.Y.Z --dir <assets>\n\
  cargo xtask release-notes --tag rtao-vX.Y.Z\n\
  cargo xtask parity check [--strict]\n\
  cargo xtask parity seed\n\
  cargo xtask core-surface [--check]\n\
  cargo xtask staged --check\n\
  cargo xtask lint-upstream\n\n\
Ordinary pin --check verifies committed pins without modifying files.\n\
With --parity --check, pin checks finalized release provenance and refreshes image digests."
            );
            Ok(())
        }
        Some(command) => bail!("unknown xtask command {command:?}; run cargo xtask --help"),
    }
}
