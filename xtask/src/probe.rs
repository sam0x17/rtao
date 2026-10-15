//! Bounded read-only observations of public chain endpoints.
//!
//! No submission or account mutation RPC is used. Calls, signed extension
//! payloads and extrinsics are built by the same core as product transactions.

use std::{
    collections::BTreeMap,
    fs,
    future::Future,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail, ensure};
use rtao_client::{BlockRef, Client, ClientConfig, Snapshot, View, ViewExt};
use rtao_core::{
    codec::{Value, value::to_corpus_json},
    digest::ChainInfo,
    extrinsic::TxParams,
    keys::{CRYPTO_ED25519, Keypair},
    ops::{metadata, runtime_api},
    value_ext::ValueExt,
};
use rtao_metagraph::{DecodePath, DecodePreference, MetagraphOptions, SubnetsMetagraphExt};
use rtao_primitives::{H256, NetUid};
use rtao_reads::Namespaces;
use rtao_rpc::{
    EndpointPool, RateLimit, ReadMethod, RpcError, Session, SessionConfig, SessionEvent,
};
use rtao_runtime::RuntimeHandle;
use rtao_testkit::rpc::RpcClient;
use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};
use toml_edit::{ArrayOfTables, DocumentMut, Table, value};

const MAX_NETWORK_SECONDS: u64 = 600;
const MAX_CHECK_SECONDS: u64 = 180;
const CHECKS: &[&str] = &[
    "finality",
    "runtime",
    "v15_digest",
    "metagraphs",
    "events",
    "compiled_metadata_hash",
];

pub fn run(args: Vec<String>) -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("xtask has no repository parent")?
        .to_owned();
    if args.first().is_some_and(|arg| arg == "pins") {
        ensure!(
            args.len() == 1 || args.as_slice() == ["pins", "--check"],
            "usage: probe pins [--check]"
        );
        return pins(&root, args.len() == 2);
    }
    pins(&root, true)?;
    let default_output = std::env::var_os("RTAO_TEMP_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"));
    let mut output = default_output.join("probe.json");
    let mut report = default_output.join("probe.md");
    let mut networks = vec!["finney".to_owned()];
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--networks" => {
                networks = args
                    .next()
                    .context("--networks requires a comma-separated list")?
                    .split(',')
                    .map(str::to_owned)
                    .collect();
                ensure!(!networks.is_empty(), "at least one network is required");
            }
            "--output" => output = args.next().context("--output requires a path")?.into(),
            "--report" => report = args.next().context("--report requires a path")?.into(),
            _ => bail!("unknown probe argument {arg:?}"),
        }
    }
    ensure!(
        output != report,
        "JSON output and Markdown report must differ"
    );
    let started = now_millis()?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let mut results = Vec::new();
    for network in networks {
        ensure!(
            matches!(network.as_str(), "finney" | "testnet" | "test" | "devnet"),
            "probe supports finney, testnet and devnet, not {network:?}"
        );
        println!("probe {network}: starting bounded read-only checks");
        let result = runtime.block_on(async {
            tokio::time::timeout(
                Duration::from_secs(MAX_NETWORK_SECONDS),
                probe_network(&root, &network),
            )
            .await
        });
        let value = match result {
            Ok(Ok(value)) => value,
            Ok(Err(error)) => unavailable(&network, &format!("{error:#}")),
            Err(_) => unavailable(&network, "network probe exceeded its 10-minute deadline"),
        };
        results.push(value);
        // Preserve completed networks even if the process is interrupted later.
        write_results(&output, &report, started, &results)?;
    }
    let failed = results.iter().any(|result| result["passed"] != true);
    ensure!(
        !failed,
        "live probe failed; inspect {} and {}",
        output.display(),
        report.display()
    );
    println!("probe passed; evidence {}", output.display());
    Ok(())
}

fn pins(root: &Path, check: bool) -> Result<()> {
    let mut document = DocumentMut::new();
    document["schema"] = value(1);
    document["recipe"] = value(
        "subtensor runtime/build.rs enable_metadata_hash(TAO, 9); metadata System.Version and SS58Prefix",
    );
    let mut rows = ArrayOfTables::new();
    for network in ["finney", "testnet", "devnet"] {
        let index = fs::read_to_string(root.join(format!("vendor/metadata/{network}/index.toml")))?
            .parse::<DocumentMut>()?;
        for row in index["runtimes"]
            .as_array_of_tables()
            .context("metadata index has no runtime rows")?
        {
            let spec = row["spec"]
                .as_integer()
                .and_then(|spec| u32::try_from(spec).ok())
                .context("metadata pin spec is not u32")?;
            let blob = row["v15"].as_table_like().context("pin has no V15")?;
            let filename = blob
                .get("path")
                .and_then(toml_edit::Item::as_str)
                .context("V15 pin has no path")?;
            ensure!(
                filename == format!("{spec}.v15.scale"),
                "V15 filename differs from spec"
            );
            let bytes = fs::read(root.join(format!("vendor/metadata/{network}/{filename}")))?;
            let sha = hex::encode(Sha256::digest(&bytes));
            ensure!(
                Some(sha.as_str()) == blob.get("sha256").and_then(toml_edit::Item::as_str),
                "V15 pin SHA256 differs"
            );
            let runtime = RuntimeHandle::parse(&bytes, spec, 1, 42)?;
            let info = chain_info(&runtime)?;
            let digest = runtime.metadata_digest(&info)?;
            let mut output = Table::new();
            output["network"] = value(network);
            output["spec"] = value(i64::from(spec));
            for field in ["genesis", "code_hash", "commit"] {
                output[field] = value(
                    row[field]
                        .as_str()
                        .with_context(|| format!("pin lacks {field}"))?,
                );
            }
            output["v15_sha256"] = value(sha);
            output["spec_name"] = value(info.spec_name);
            output["base58_prefix"] = value(i64::from(info.base58_prefix));
            output["decimals"] = value(i64::from(info.decimals));
            output["token_symbol"] = value(info.token_symbol);
            output["rfc0078_digest"] = value(format!("0x{}", hex::encode(digest)));
            rows.push(output);
        }
    }
    document["runtimes"] = toml_edit::Item::ArrayOfTables(rows);
    let path = root.join("codegen/probe-pins.toml");
    let generated =
        format!("# @generated by cargo xtask probe pins; immutable RFC-0078 inputs.\n{document}");
    if check {
        ensure!(
            fs::read_to_string(&path).context("probe pins missing; run cargo xtask probe pins")?
                == generated,
            "probe pins are stale; run cargo xtask probe pins"
        );
    } else {
        fs::write(&path, generated)?;
    }
    Ok(())
}

fn unavailable(network: &str, detail: &str) -> Json {
    json!({"network":network,"passed":false,"checks":CHECKS.iter().map(|name|
        json!({"name":name,"status":"unavailable","error":detail})).collect::<Vec<_>>()})
}

fn now_millis() -> Result<u64> {
    u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())
        .context("wall clock milliseconds exceed u64")
}

fn write_results(output: &Path, report: &Path, started: u64, results: &[Json]) -> Result<()> {
    for path in [output, report] {
        if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
    }
    let document = json!({"schema":1,"started_unix_ms":started,"finished_unix_ms":now_millis()?,
        "read_only":true,"python_used":false,"networks":results});
    fs::write(
        output,
        format!("{}\n", serde_json::to_string_pretty(&document)?),
    )?;
    let mut markdown =
        format!("Live probe started at Unix millisecond {started}. All RPCs were read-only.\n\n");
    for network in results {
        markdown.push_str(&format!(
            "Network `{}`: {}.\n\n",
            network["network"].as_str().unwrap_or("?"),
            if network["passed"] == true {
                "passed"
            } else {
                "failed"
            }
        ));
        if let Some(checks) = network["checks"].as_array() {
            for check in checks {
                markdown.push_str(&format!(
                    "- `{}`: {}{}\n",
                    check["name"].as_str().unwrap_or("?"),
                    check["status"].as_str().unwrap_or("?"),
                    check["error"]
                        .as_str()
                        .map(|error| format!(" — {error}"))
                        .unwrap_or_default()
                ));
            }
        }
        markdown.push('\n');
    }
    markdown.push_str(&format!(
        "Machine-readable observations: `{}`.\n",
        output.display()
    ));
    fs::write(report, markdown)?;
    Ok(())
}

async fn check<F: Future<Output = Result<Json>>>(network: &str, name: &str, task: F) -> Json {
    let started = tokio::time::Instant::now();
    let result = tokio::time::timeout(Duration::from_secs(MAX_CHECK_SECONDS), task).await;
    let value = match result {
        Ok(Ok(value)) if value.get("failure").is_some() => {
            json!({"name":name,"status":"failed","error":value["failure"],"observations":value})
        }
        Ok(Ok(value)) => json!({"name":name,"status":"passed","observations":value}),
        Ok(Err(error)) => json!({"name":name,"status":"failed","error":format!("{error:#}")}),
        Err(_) => {
            json!({"name":name,"status":"failed","error":"check exceeded its 180-second deadline"})
        }
    };
    println!(
        "probe {network}: {name} {} ({:.1}s)",
        value["status"].as_str().unwrap_or("?"),
        started.elapsed().as_secs_f64()
    );
    value
}

fn passive_config(url: url::Url) -> SessionConfig {
    SessionConfig {
        pool: EndpointPool::new(url),
        rate_limit: Some(RateLimit {
            per_second: 1.0,
            burst: 1,
        }),
        max_in_flight: 1,
        max_retries: 1,
        retry_forever: false,
        handshake_timeout: Duration::from_secs(10),
        silence_timeout: Duration::from_secs(30),
        max_message_bytes: 20 * 1024 * 1024,
        ..SessionConfig::default()
    }
}

fn error_observation(error: &RpcError) -> Json {
    match error {
        RpcError::Policy(policy) => {
            json!({"outcome":"policy_refusal","kind":format!("{:?}",policy.kind),
            "status_code":policy.status_code,"retry_after":policy.retry_after,"policy":policy.policy,
            "reason":policy.reason,"message":policy.message})
        }
        _ => json!({"outcome":"error","message":error.to_string()}),
    }
}

async fn probe_network(root: &Path, label: &str) -> Result<Json> {
    let named = if label == "testnet" { "test" } else { label };
    let canonical = if label == "test" { "testnet" } else { label };
    let mut config = ClientConfig::for_network(named)?;
    config.runtime_cache_dir = None;
    config.config_store = None;
    config.track_heads = false;
    config.head_policy = rtao_client::HeadPolicy::Finalized;
    config.startup_timeout = Duration::from_secs(90);
    let pool = config.network.endpoint_pool()?;
    let mut observations = Vec::new();
    let mut selected = None;
    let mut endpoints = BTreeMap::new();
    for (role, urls) in [("ordinary", pool.urls), ("archive", pool.archive)] {
        for url in urls {
            endpoints
                .entry(url.as_str().to_owned())
                .or_insert((role, url));
        }
    }
    ensure!(
        endpoints.len() <= 12,
        "endpoint catalog exceeds bounded passive scope"
    );
    for (_, (role, url)) in endpoints {
        let endpoint = rtao_rpc::policy::safe_endpoint(url.as_str());
        let settings = passive_config(url);
        let observation = match tokio::time::timeout(
            Duration::from_secs(45),
            Session::connect(settings.clone()),
        )
        .await
        {
            Ok(Ok(session)) => {
                let health = session
                    .request::<Json>(ReadMethod::SystemHealth, json!([]))
                    .await;
                let methods = session
                    .request::<Json>(ReadMethod::RpcMethods, json!([]))
                    .await;
                let observation = json!({"endpoint":endpoint,"role":role,"connected":true,
                    "health":health.as_ref().map_or_else(error_observation,|value|json!({"outcome":"accepted","value":value})),
                    "rpc_methods":methods.as_ref().map_or_else(error_observation,|value|json!({"outcome":"accepted","value":value}))});
                if health.is_ok() && methods.is_ok() && role == "ordinary" && selected.is_none() {
                    selected = Some((session, settings, endpoint.clone()));
                } else {
                    session.close().await;
                }
                observation
            }
            Ok(Err(error)) => {
                json!({"endpoint":endpoint,"role":role,"connected":false,"refusal":error_observation(&error)})
            }
            Err(_) => {
                json!({"endpoint":endpoint,"role":role,"connected":false,"error":"handshake exceeded 45-second deadline"})
            }
        };
        println!("probe {label}: observed {endpoint}");
        observations.push(observation);
    }
    let Some((session, settings, endpoint)) = selected else {
        let mut value = unavailable(
            label,
            "no ordinary endpoint accepted the passive read checks",
        );
        value["endpoint_policy"] = json!(observations);
        return Ok(value);
    };
    config.session = settings;
    let mut events = session.events();
    let client = match Client::from_rpc(RpcClient::new(session.clone()), config).await {
        Ok(client) => client,
        Err(error) => {
            session.close().await;
            let mut value = unavailable(label, &error.to_string());
            value["endpoint_policy"] = json!(observations);
            return Ok(value);
        }
    };
    let snapshot = client.at(BlockRef::Finalized).await?;
    let mut checks = Vec::new();
    checks.push(check(label, "finality", finality(&client, &snapshot)).await);
    checks.push(
        check(
            label,
            "runtime",
            runtime_identity(root, canonical, &client, &snapshot, &session),
        )
        .await,
    );
    checks.push(
        check(
            label,
            "v15_digest",
            v15_digest(root, canonical, &snapshot, &session),
        )
        .await,
    );
    checks.push(check(label, "metagraphs", metagraphs(&snapshot)).await);
    checks.push(check(label, "events", last_events(&client, &snapshot)).await);
    checks.push(
        check(
            label,
            "compiled_metadata_hash",
            compiled_hash(&client, &snapshot),
        )
        .await,
    );
    let mut session_events = Vec::new();
    while let Ok(event) = events.try_recv() {
        session_events.push(match event {
            SessionEvent::PolicyRefused(policy) => {
                error_observation(&RpcError::Policy(Box::new(policy)))
            }
            event => json!({"outcome":"session_event","event":format!("{event:?}")}),
        });
    }
    session.close().await;
    Ok(
        json!({"network":canonical,"endpoint":endpoint,"passed":checks.iter().all(|check|check["status"]=="passed"),
        "state_hash":hash_hex(snapshot.block().hash),"block":snapshot.block().number,"checks":checks,
        "endpoint_policy":{"pacing_requests_per_second":1,"burst":1,"active_limit_search":false,
            "endpoints":observations,"session_events":session_events}}),
    )
}

fn hash_hex(hash: H256) -> String {
    format!("0x{}", hex::encode(hash.0))
}

async fn finality(client: &Client, snapshot: &Snapshot) -> Result<Json> {
    let best = client.at(BlockRef::Best).await?;
    let finalized_time = snapshot
        .timestamp()
        .await?
        .duration_since(UNIX_EPOCH)?
        .as_millis();
    let best_time = best
        .timestamp()
        .await?
        .duration_since(UNIX_EPOCH)?
        .as_millis();
    let now = u128::from(now_millis()?);
    ensure!(
        finalized_time <= now.saturating_add(30_000),
        "finalized timestamp is over 30 seconds in the future"
    );
    let age = now.saturating_sub(finalized_time);
    let lag = best_time.saturating_sub(finalized_time);
    ensure!(
        age < 600_000 && lag < 600_000,
        "finality age {age}ms / best-to-finalized lag {lag}ms exceeds 10 minutes"
    );
    Ok(
        json!({"threshold_ms":600000,"wall_clock_age_ms":age,"best_to_finalized_ms":lag,
        "finalized_timestamp_ms":finalized_time,"best_timestamp_ms":best_time,"observed_unix_ms":now,
        "best_block":best.block().number,"finalized_block":snapshot.block().number}),
    )
}

fn pin_row(root: &Path, network: &str, spec: u32) -> Result<toml_edit::Table> {
    let path = root.join(format!("vendor/metadata/{network}/index.toml"));
    let document = fs::read_to_string(&path)?.parse::<DocumentMut>()?;
    ensure!(
        document["schema"].as_integer() == Some(1),
        "metadata index schema is not 1"
    );
    ensure!(
        document["network"].as_str() == Some(network),
        "metadata index network differs"
    );
    document["runtimes"]
        .as_array_of_tables()
        .context("metadata index has no runtime rows")?
        .iter()
        .find(|row| row["spec"].as_integer() == Some(i64::from(spec)))
        .cloned()
        .with_context(|| format!("live spec {spec} has no immutable {network} pin"))
}

async fn runtime_identity(
    root: &Path,
    network: &str,
    client: &Client,
    snapshot: &Snapshot,
    session: &Session,
) -> Result<Json> {
    let at = hash_hex(snapshot.block().hash);
    let version = session
        .request::<Json>(ReadMethod::StateGetRuntimeVersion, json!([at]))
        .await?;
    let spec = version["specVersion"]
        .as_u64()
        .and_then(|spec| u32::try_from(spec).ok())
        .context("specVersion is not u32")?;
    ensure!(
        spec == snapshot.block().runtime.spec_version(),
        "parent-selected runtime differs from finalized state runtime; probe again after upgrade"
    );
    let row = pin_row(root, network, spec)?;
    ensure!(
        row["genesis"].as_str() == Some(hash_hex(client.genesis()).as_str()),
        "genesis differs from immutable pin"
    );
    let code_hash = session
        .request::<Json>(ReadMethod::StateGetStorageHash, json!(["0x3a636f6465", at]))
        .await?;
    ensure!(
        code_hash.as_str() == row["code_hash"].as_str(),
        "live :code hash {code_hash} differs from immutable spec pin {:?}",
        row["code_hash"].as_str()
    );
    let properties = session
        .request::<Json>(ReadMethod::SystemProperties, json!([]))
        .await?;
    ensure!(
        properties["ss58Format"].as_u64() == Some(42),
        "live SS58 prefix differs from digest pin"
    );
    // Node display properties may use a testnet symbol. The runtime build's
    // enable_metadata_hash("TAO", 9) constants define RFC-0078 instead; those
    // immutable values are recorded independently in codegen/probe-pins.toml.
    Ok(
        json!({"version":version,"code_hash":code_hash,"genesis":hash_hex(client.genesis()),"properties":properties,
        "pin_tag":row["tag"].as_str(),"pin_commit":row["commit"].as_str()}),
    )
}

fn chain_info(runtime: &RuntimeHandle) -> Result<ChainInfo> {
    let constant = runtime
        .core()
        .constant("System", "Version")
        .context("System.Version is absent")?;
    let version = runtime.core().decode_spec(
        &rtao_core::runtime::TypeSpec::Id(constant.ty),
        &constant.value,
        true,
    )?;
    let spec_name = version
        .field("spec_name")
        .and_then(Value::as_str)
        .context("System.Version.spec_name is absent")?;
    ensure!(
        version.field("spec_version").and_then(Value::as_u128)
            == Some(u128::from(runtime.spec_version())),
        "metadata System.Version.spec_version differs from the runtime binding"
    );
    let prefix = runtime
        .core()
        .constant("System", "SS58Prefix")
        .context("System.SS58Prefix is absent")?;
    let prefix = runtime.core().decode_spec(
        &rtao_core::runtime::TypeSpec::Id(prefix.ty),
        &prefix.value,
        true,
    )?;
    ensure!(
        prefix.as_u128() == Some(42),
        "metadata System.SS58Prefix differs from digest recipe"
    );
    Ok(ChainInfo {
        spec_version: runtime.spec_version(),
        spec_name: spec_name.to_owned(),
        base58_prefix: 42,
        decimals: 9,
        token_symbol: "TAO".into(),
    })
}

async fn v15_digest(
    root: &Path,
    network: &str,
    snapshot: &Snapshot,
    session: &Session,
) -> Result<Json> {
    let request = metadata::v15_request(Some(&hash_hex(snapshot.block().hash)));
    let result = session
        .request::<Json>(ReadMethod::StateCall, request.params)
        .await
        .map_err(|error| {
            anyhow::anyhow!(
                "V15 request failed (missing-method needle={}): {error}",
                error
                    .to_string()
                    .contains(rtao_catalog::runtime_py::V15_MISSING_NEEDLE)
            )
        })?;
    let raw = metadata::decode_v15(&result)?.context("V15 unavailable: runtime returned None")?;
    ensure!(
        raw.get(..5) == Some(b"meta\x0f".as_slice()),
        "runtime did not return V15 metadata"
    );
    let runtime = &snapshot.block().runtime;
    ensure!(
        raw == runtime.core().metadata_bytes,
        "served V15 differs from product runtime-cache bytes"
    );
    let row = pin_row(root, network, runtime.spec_version())?;
    let blob = row["v15"].as_table_like().context("pin has no V15 blob")?;
    let path = blob
        .get("path")
        .and_then(toml_edit::Item::as_str)
        .context("pin has no V15 filename")?;
    ensure!(
        path == format!("{}.v15.scale", runtime.spec_version()),
        "noncanonical V15 pin path"
    );
    let pinned = fs::read(root.join(format!("vendor/metadata/{network}/{path}")))?;
    let pinned_sha = hex::encode(Sha256::digest(&pinned));
    ensure!(
        Some(pinned_sha.as_str()) == blob.get("sha256").and_then(toml_edit::Item::as_str),
        "pinned V15 SHA256 differs"
    );
    let info = chain_info(runtime)?;
    let pinned_runtime = RuntimeHandle::parse(
        &pinned,
        runtime.spec_version(),
        runtime.transaction_version(),
        42,
    )?;
    let expected = pinned_runtime.metadata_digest(&chain_info(&pinned_runtime)?)?;
    let observed = runtime.metadata_digest(&info)?;
    let digest_pins =
        fs::read_to_string(root.join("codegen/probe-pins.toml"))?.parse::<DocumentMut>()?;
    let digest_pin = digest_pins["runtimes"]
        .as_array_of_tables()
        .context("digest pin has no runtime rows")?
        .iter()
        .find(|row| {
            row["network"].as_str() == Some(network)
                && row["spec"].as_integer() == Some(i64::from(runtime.spec_version()))
        })
        .context("live runtime has no immutable RFC-0078 digest pin")?;
    ensure!(
        digest_pin["genesis"].as_str() == row["genesis"].as_str()
            && digest_pin["code_hash"].as_str() == row["code_hash"].as_str(),
        "RFC-0078 digest pin chain/code binding differs"
    );
    ensure!(
        digest_pin["rfc0078_digest"].as_str()
            == Some(format!("0x{}", hex::encode(expected)).as_str()),
        "recomputed pinned RFC-0078 digest differs from immutable digest row"
    );
    ensure!(
        expected == observed,
        "live V15 RFC-0078 digest {} differs from pinned V15 digest {}",
        hex::encode(observed),
        hex::encode(expected)
    );
    Ok(
        json!({"available":true,"missing_method_needle":false,"bytes":raw.len(),
        "live_sha256":hex::encode(Sha256::digest(&raw)),"pinned_sha256":pinned_sha,
        "rfc0078_digest":format!("0x{}",hex::encode(observed)),"pinned_rfc0078_digest":format!("0x{}",hex::encode(expected)),
        "chain_info":{"spec_version":info.spec_version,"spec_name":info.spec_name,"base58_prefix":info.base58_prefix,
            "decimals":info.decimals,"token_symbol":info.token_symbol}}),
    )
}

async fn populations(snapshot: &Snapshot) -> Result<BTreeMap<NetUid, u64>> {
    let runtime = &snapshot.block().runtime;
    let active = runtime
        .storage_entry("SubtensorModule", "NetworksAdded")
        .context("NetworksAdded is absent")?;
    let population = runtime
        .storage_entry("SubtensorModule", "SubnetworkN")
        .context("SubnetworkN is absent")?;
    let prefix = runtime.core().storage_key(active, &[])?;
    let mut start = None;
    let mut result = BTreeMap::new();
    loop {
        let keys = snapshot.keys_paged(&prefix, 128, start.as_deref()).await?;
        if keys.is_empty() {
            break;
        }
        ensure!(
            result.len() + keys.len() <= 8192,
            "subnet map exceeds bounded probe population limit"
        );
        let values = snapshot.storage_raw_many(&keys).await?;
        let mut candidates = Vec::new();
        let mut population_keys = Vec::new();
        for (key, raw) in keys.iter().zip(values) {
            let enabled = rtao_core::ops::storage::decode_storage_bytes(
                runtime.core(),
                active,
                raw.as_deref(),
            )?;
            if enabled.as_bool() != Some(true) {
                continue;
            }
            let params = runtime.core().decode_storage_key_params(active, key, 0)?;
            let raw = params
                .first()
                .and_then(Value::as_u128)
                .and_then(|raw| u16::try_from(raw).ok())
                .context("NetworksAdded key is not u16")?;
            let netuid = NetUid::new(raw)?;
            population_keys.push(runtime.core().storage_key(population, &params)?);
            candidates.push(netuid);
        }
        let values = snapshot.storage_raw_many(&population_keys).await?;
        for (netuid, raw) in candidates.into_iter().zip(values) {
            let count = rtao_core::ops::storage::decode_storage_bytes(
                runtime.core(),
                population,
                raw.as_deref(),
            )?
            .as_u128()
            .and_then(|value| u64::try_from(value).ok())
            .context("SubnetworkN is not u64")?;
            ensure!(
                result.insert(netuid, count).is_none(),
                "duplicate subnet key in paging result"
            );
        }
        let next = keys.last().cloned();
        ensure!(next != start, "subnet map page cursor failed to advance");
        start = next;
    }
    ensure!(!result.is_empty(), "no registered subnets were found");
    Ok(result)
}

async fn metagraphs(snapshot: &Snapshot) -> Result<Json> {
    let populations = populations(snapshot).await?;
    let largest = populations
        .iter()
        .max_by_key(|(netuid, count)| (**count, std::cmp::Reverse(**netuid)))
        .map(|(netuid, _)| *netuid)
        .context("no largest subnet")?;
    let mut result = Vec::new();
    for netuid in [NetUid(1), largest] {
        let mut baseline = None;
        let mut paths = Vec::new();
        for (decoder, expected, name) in [
            (DecodePreference::View, DecodePath::View, "view"),
            (DecodePreference::Typed, DecodePath::Typed, "typed"),
            (DecodePreference::Dynamic, DecodePath::Dynamic, "dynamic"),
        ] {
            let options = MetagraphOptions {
                commitments: false,
                view: true,
                decoder,
            };
            let graph = snapshot
                .subnets()
                .metagraph_with(netuid, options)
                .await?
                .context("registered subnet returned no graph")?;
            ensure!(
                graph.decode_path() == expected,
                "{name} path silently fell back"
            );
            let json = graph.to_json()?;
            if let Some(baseline) = &baseline {
                ensure!(
                    *baseline == json,
                    "{name} JSON differs from other metagraph paths"
                );
            }
            let serialized = serde_json::to_vec(&json)?;
            paths.push(json!({"decoder":name,"uids":graph.len(),"json_sha256":hex::encode(Sha256::digest(serialized))}));
            baseline = Some(json);
        }
        result.push(json!({"netuid":netuid.0,"paths":paths,"json_equal":true}));
    }
    // Exercise the exact public accessor with its default commitment behavior.
    let curated = snapshot
        .subnets()
        .metagraph(NetUid(1))
        .await?
        .context("curated subnet 1 read returned None")?;
    Ok(
        json!({"largest_subnet":largest.0,"population_scan":populations.iter().map(|(netuid,count)|
        json!({"netuid":netuid.0,"population":count})).collect::<Vec<_>>(),"graphs":result,
        "curated_default":{"netuid":1,"uids":curated.len(),"commitments_fetched":curated.commitments.is_some()}}),
    )
}

async fn last_events(client: &Client, snapshot: &Snapshot) -> Result<Json> {
    let mut current = snapshot.clone();
    let mut blocks = Vec::new();
    for _ in 0..10 {
        let events = current.events().await?;
        let identities = events
            .raw()
            .iter()
            .map(|event| format!("{}.{}", event.pallet(), event.name()))
            .collect::<Vec<_>>();
        blocks.push(json!({"block":current.block().number,"hash":hash_hex(current.block().hash),
            "spec_version":current.block().runtime.spec_version(),"events":events.records().len(),"identities":identities}));
        if blocks.len() == 10 {
            break;
        }
        let header = request_client(
            client,
            "chain_getHeader",
            json!([hash_hex(current.block().hash)]),
        )
        .await?;
        let parent = header["parentHash"]
            .as_str()
            .context("block has no parentHash")?;
        let parent = hex::decode(
            parent
                .strip_prefix("0x")
                .context("parent hash lacks 0x prefix")?,
        )?;
        let parent = H256(
            parent
                .try_into()
                .map_err(|_| anyhow::anyhow!("parent hash is not 32 bytes"))?,
        );
        current = client.at(BlockRef::Hash(parent)).await?;
    }
    Ok(
        json!({"blocks":blocks,"count":10,"runtime_selection":"parent runtime for each immutable block"}),
    )
}

async fn request_client(client: &Client, method: &str, params: Json) -> Result<Json> {
    let mut encoded = rtao_testkit::rpc::client::RpcParams::new();
    for param in params.as_array().context("RPC params must be array")? {
        encoded.push(param)?;
    }
    Ok(client.rpc().request(method, encoded).await?)
}

fn metadata_validation_request(
    runtime: &RuntimeHandle,
    genesis: H256,
    at: H256,
) -> Result<(Vec<u8>, Json)> {
    runtime.signing_status()?;
    let info = chain_info(runtime)?;
    let digest = runtime.metadata_digest(&info)?;
    let mnemonic = Keypair::generate_mnemonic(12)?;
    let key = Keypair::from_mnemonic(&mnemonic, CRYPTO_ED25519, None)?;
    let public = key.public_key_bytes();
    let call = rtao_runtime::calls::balances::transfer_keep_alive(
        rtao_core::DynValue::new(Value::Bytes(public.to_vec())),
        rtao_primitives::TaoBalance(1),
    )
    .compose(runtime)?;
    let params = TxParams {
        era: Value::str("00"),
        nonce: 0,
        tip: 0,
        tip_asset_id: None,
        genesis_hash: genesis.0,
        era_block_hash: genesis.0,
        metadata_hash: Some(digest),
    };
    let payload = runtime.core().signature_payload(call.as_bytes(), &params)?;
    let signature = key.sign(&payload)?;
    ensure!(
        rtao_core::keys::verify_strict(
            &payload,
            &signature,
            &rtao_core::keys::ss58_from_public(public, 42),
            CRYPTO_ED25519
        )?,
        "ephemeral ed25519 signature failed local verification"
    );
    let (extrinsic, extrinsic_hash) = runtime.core().encode_signed_extrinsic(
        call.as_bytes(),
        public,
        &signature,
        CRYPTO_ED25519,
        &params,
    )?;
    let method = runtime_api::runtime_api_method(
        runtime.core(),
        "TaggedTransactionQueue",
        "validate_transaction",
    )?;
    ensure!(
        method
            .inputs
            .iter()
            .map(|input| input.name.as_str())
            .eq(["source", "tx", "block_hash"]),
        "TaggedTransactionQueue API is not the three-argument v3 contract"
    );
    let source = method
        .inputs
        .first()
        .context("validate_transaction source is absent")?;
    let hash = method
        .inputs
        .get(2)
        .context("validate_transaction block_hash is absent")?;
    let mut encoded = Vec::new();
    runtime
        .core()
        .encode_id(source.ty, &Value::str("External"), &mut encoded)?;
    // UncheckedExtrinsic::Encode includes its compact length; the core has
    // already produced the complete wire value, so append it without a second
    // Vec length. This is the v3 runtime API's tx argument, never submitted.
    encoded.extend_from_slice(&extrinsic);
    runtime
        .core()
        .encode_id(hash.ty, &Value::Bytes(at.0.to_vec()), &mut encoded)?;
    Ok((
        encoded,
        json!({"account":rtao_core::keys::ss58_from_public(public,42),"crypto":"ed25519",
        "call":"Balances.transfer_keep_alive","rao":1,"nonce":0,"era":"immortal",
        "metadata_hash_mode":"Enabled","metadata_digest":format!("0x{}",hex::encode(digest)),
        "extrinsic_hash":format!("0x{}",hex::encode(extrinsic_hash)),"extrinsic_hex":format!("0x{}",hex::encode(extrinsic)),
        "signature_locally_verified":true,"submission":false}),
    ))
}

fn check_validity(value: &Value) -> Result<()> {
    if let Some(error) = value.variant("Err") {
        let (kind, payload) = if let Some(payload) = error.variant("Unknown") {
            ("Unknown", payload)
        } else if let Some(payload) = error.variant("Invalid") {
            ("Invalid", payload)
        } else {
            bail!("unrecognized TransactionValidityError shape");
        };
        let name = payload
            .variant_name()
            .context("transaction validity error has no variant")?;
        ensure!(
            !(kind == "Unknown" && name == "CannotLookup"),
            "runtime has no compiled metadata hash: UnknownTransaction::CannotLookup"
        );
        ensure!(
            !(kind == "Invalid" && name == "BadProof"),
            "compiled metadata hash differs: InvalidTransaction::BadProof"
        );
    } else {
        ensure!(
            value.variant("Ok").is_some(),
            "unrecognized TransactionValidity result shape"
        );
    }
    Ok(())
}

async fn compiled_hash(client: &Client, snapshot: &Snapshot) -> Result<Json> {
    let runtime = &snapshot.block().runtime;
    let (args, mut evidence) =
        metadata_validation_request(runtime, client.genesis(), snapshot.block().hash)?;
    evidence["validation_args_hex"] = json!(format!("0x{}", hex::encode(&args)));
    let raw = match snapshot
        .state_call("TaggedTransactionQueue_validate_transaction", &args)
        .await
    {
        Ok(raw) => raw,
        Err(error) => {
            evidence["failure"] = json!(error.to_string());
            return Ok(evidence);
        }
    };
    evidence["validation_response_hex"] = json!(format!("0x{}", hex::encode(&raw)));
    let method = runtime_api::runtime_api_method(
        runtime.core(),
        "TaggedTransactionQueue",
        "validate_transaction",
    )?;
    let value = match runtime_api::decode_bytes(runtime.core(), method, &raw) {
        Ok(value) => value,
        Err(error) => {
            evidence["failure"] = json!(error.to_string());
            return Ok(evidence);
        }
    };
    if let Err(error) = check_validity(&value) {
        evidence["failure"] = json!(error.to_string());
    }
    evidence["validation_result"] = to_corpus_json(&value);
    Ok(evidence)
}

#[cfg(test)]
mod tests {
    use super::*;
    use parity_scale_codec::DecodeAll;
    const METADATA: &[u8] = include_bytes!("../../vendor/metadata/finney/478.v15.scale");

    fn runtime() -> Result<RuntimeHandle> {
        Ok(RuntimeHandle::parse(METADATA, 478, 1, 42)?)
    }

    #[test]
    fn refuses_digest_inputs_bound_to_the_wrong_metadata_spec() -> Result<()> {
        let correctly_bound = runtime()?;
        assert_eq!(chain_info(&correctly_bound)?.spec_version, 478);
        let wrongly_bound = RuntimeHandle::parse(METADATA, 477, 1, 42)?;
        assert!(chain_info(&wrongly_bound).is_err());
        Ok(())
    }

    #[test]
    fn rejects_real_metadata_hash_failure_wire_variants_and_trailing_bytes() -> Result<()> {
        let runtime = runtime()?;
        let method = runtime_api::runtime_api_method(
            runtime.core(),
            "TaggedTransactionQueue",
            "validate_transaction",
        )?;
        for raw in [vec![1, 1, 0], vec![1, 0, 4]] {
            let value = runtime_api::decode_bytes(runtime.core(), method, &raw)?;
            assert!(check_validity(&value).is_err());
        }
        for raw in [vec![1, 0, 1], vec![1, 0, 7, 17], vec![1, 1, 2, 42]] {
            let value = runtime_api::decode_bytes(runtime.core(), method, &raw)?;
            check_validity(&value)?;
        }
        assert!(runtime_api::decode_bytes(runtime.core(), method, &[1, 0, 1, 0]).is_err());
        assert!(runtime_api::decode_bytes(runtime.core(), method, &[1, 0, 7]).is_err());
        Ok(())
    }

    #[test]
    fn enabled_ephemeral_probe_matches_core_extrinsic_and_api_argument_encoding() -> Result<()> {
        let runtime = runtime()?;
        let genesis = H256([1; 32]);
        let at = H256([2; 32]);
        let (encoded, evidence) = metadata_validation_request(&runtime, genesis, at)?;
        let extrinsic = hex::decode(
            evidence["extrinsic_hex"]
                .as_str()
                .context("missing extrinsic")?
                .trim_start_matches("0x"),
        )?;
        let mut expected = vec![2];
        expected.extend_from_slice(&extrinsic);
        expected.extend_from_slice(&at.0);
        assert_eq!(encoded, expected);
        let decoded = runtime.core().decode_extrinsic(&extrinsic, true)?;
        assert_eq!(
            decoded.field("address").and_then(Value::as_str),
            evidence["account"].as_str()
        );
        assert_eq!(evidence["metadata_hash_mode"], "Enabled");
        assert_eq!(
            decoded
                .field("mode")
                .and_then(|mode| mode.field("mode"))
                .and_then(Value::as_str),
            Some("Enabled")
        );
        assert_eq!(evidence["submission"], false);
        assert!(
            runtime
                .core()
                .extrinsic
                .signed_extensions
                .iter()
                .any(|extension| extension.identifier == "CheckMetadataHash")
        );
        // The encoded UncheckedExtrinsic length is included exactly once.
        let body = Vec::<u8>::decode_all(&mut extrinsic.as_slice())?;
        assert_eq!(body.first().copied(), Some(0x84));
        Ok(())
    }
}
