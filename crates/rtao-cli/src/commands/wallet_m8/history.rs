// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/wallet.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Bounded mainnet transfer-indexer access with same-origin HTTPS pagination.

use super::*;
use std::{collections::BTreeSet, time::Duration};
use url::Url;

const TRANSFERS_API: &str = "https://api.taomarketcap.com/internal/v1/transactions/transfers/";
const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_PAGES: usize = 128;

trait Transport: Send + Sync {
    fn get<'a>(&'a self, url: &'a Url) -> rtao::signer::BoxFuture<'a, Result<Vec<u8>, CliError>>;
}

struct Http(reqwest::Client);
impl Http {
    fn new() -> Result<Self, CliError> {
        let mut roots = rustls::RootCertStore::empty();
        let (added, _) =
            roots.add_parsable_certificates(rustls_native_certs::load_native_certs().certs);
        if added == 0 {
            return Err(failure("no usable native TLS roots for transfer history"));
        }
        let mut tls = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(failure)?
        .with_root_certificates(roots)
        .with_no_client_auth();
        tls.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        let http = reqwest::Client::builder()
            .tls_backend_preconfigured(tls)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(failure)?;
        Ok(Self(http))
    }
}
impl Transport for Http {
    fn get<'a>(&'a self, url: &'a Url) -> rtao::signer::BoxFuture<'a, Result<Vec<u8>, CliError>> {
        Box::pin(async move {
            let mut response = self
                .0
                .get(url.clone())
                .header("Accept", "application/json")
                .send()
                .await
                .map_err(|_| failure("could not fetch transfer history"))?
                .error_for_status()
                .map_err(|_| failure("transfer indexer returned an unsuccessful status"))?;
            if response
                .content_length()
                .is_some_and(|size| size > MAX_BYTES as u64)
            {
                return Err(failure("transfer history exceeds 16 MiB"));
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| failure("could not read transfer history"))?
            {
                if bytes.len().saturating_add(chunk.len()) > MAX_BYTES {
                    return Err(failure("transfer history exceeds 16 MiB"));
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(bytes)
        })
    }
}

fn next_url(value: &str) -> Result<Url, CliError> {
    let mut url = Url::parse(value).map_err(|_| failure("indexer pagination URL is invalid"))?;
    if url.scheme() == "http" {
        url.set_scheme("https")
            .map_err(|_| failure("invalid pagination scheme"))?;
    }
    if url.scheme() != "https"
        || url.host_str() != Some("api.taomarketcap.com")
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.path() != "/internal/v1/transactions/transfers/"
    {
        return Err(failure(
            "refusing an indexer pagination URL outside its HTTPS transfers endpoint",
        ));
    }
    Ok(url)
}

async fn fetch(transport: &dyn Transport, owner: &str, limit: i64) -> Result<Vec<Value>, CliError> {
    if limit <= 0 {
        return Ok(Vec::new());
    }
    let limit =
        usize::try_from(limit).map_err(|_| input("history limit exceeds this platform's range"))?;
    let mut first = Url::parse(TRANSFERS_API).map_err(failure)?;
    first
        .query_pairs_mut()
        .append_pair("coldkey", owner)
        .append_pair("limit", &limit.min(200).to_string());
    let mut next = Some(first);
    let mut seen = BTreeSet::new();
    let mut bytes_seen = 0usize;
    let mut rows = Vec::new();
    while let Some(url) = next {
        if !seen.insert(url.to_string()) {
            return Err(failure("indexer pagination repeated a page"));
        }
        if seen.len() > MAX_PAGES {
            return Err(failure("transfer history exceeds its 128-page budget"));
        }
        let bytes = transport.get(&url).await?;
        bytes_seen = bytes_seen.saturating_add(bytes.len());
        if bytes_seen > MAX_BYTES {
            return Err(failure("transfer history exceeds 16 MiB"));
        }
        let body: Value = serde_json::from_slice(&bytes)
            .map_err(|_| failure("transfer indexer returned invalid JSON"))?;
        let object = body
            .as_object()
            .ok_or_else(|| failure("transfer indexer response is not an object"))?;
        match object.get("results") {
            None => {}
            Some(Value::Array(values)) => {
                for row in values.iter().take(limit - rows.len()) {
                    if !row.is_object() {
                        return Err(failure("transfer indexer row is not an object"));
                    }
                    rows.push(row.clone());
                }
            }
            _ => return Err(failure("transfer indexer results is not an array")),
        }
        if rows.len() == limit {
            break;
        }
        next = match object.get("next") {
            None | Some(Value::Null) => None,
            Some(Value::String(value)) if value.is_empty() => None,
            Some(Value::String(value)) => Some(next_url(value)?),
            _ => return Err(failure("transfer indexer next page is not a URL")),
        };
    }
    Ok(rows)
}

fn record(owner: &str, raw: &Value) -> Value {
    let sender = &raw["from_coldkey"];
    let receiver = &raw["to_coldkey"];
    let direction = if sender == receiver {
        "self"
    } else if sender.as_str() == Some(owner) {
        "out"
    } else {
        "in"
    };
    let netuid = if direction == "in" {
        &raw["to_netuid"]
    } else {
        &raw["from_netuid"]
    };
    let truthy = match &raw["success"] {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64() != Some(0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    };
    json!({
        "block_number": raw["block_number"], "extrinsic_idx": raw["extrinsic_idx"], "timestamp": raw["timestamp"],
        "amount_rao": raw["amount"], "netuid": netuid, "from": sender, "to": receiver,
        "direction": direction, "success": truthy, "tao_price_usd": raw["tao_price_usd"],
    })
}

pub(in crate::commands::wallet) async fn run(
    ctx: &mut AppContext,
    matches: &ArgMatches,
    history: &mut PromptHistory,
) -> Result<(), CliError> {
    run_with_transport(ctx, matches, history, None).await
}

async fn run_with_transport(
    ctx: &mut AppContext,
    matches: &ArgMatches,
    history: &mut PromptHistory,
    transport: Option<&dyn Transport>,
) -> Result<(), CliError> {
    let network = rtao::client::Network::resolve(&ctx.globals.network).map_err(failure)?;
    if !matches!(network.label(), "finney" | "archive") {
        return Err(failure(format!(
            "transfer history is not indexed for network {:?}",
            network.label()
        ))
        .with_help("the taomarketcap.com indexer only tracks mainnet (finney)"));
    }
    let owner = ctx
        .resolve_target("coldkey_ss58", text(matches, "coldkey_ss58"), history)?
        .ok_or_else(|| input("missing coldkey"))?;
    let limit = matches.get_one::<i64>("limit").copied().unwrap_or(50);
    let raw = if limit <= 0 {
        Vec::new()
    } else {
        let http;
        let transport = if let Some(transport) = transport {
            transport
        } else {
            http = Http::new()?;
            &http
        };
        tokio::time::timeout(
            Duration::from_secs(30),
            fetch(transport, &owner.to_string(), limit),
        )
        .await
        .map_err(|_| failure("transfer history request timed out"))??
    };
    let records: Vec<_> = raw
        .iter()
        .map(|raw| record(&owner.to_string(), raw))
        .collect();
    for row in super::super::list_records(&ctx.globals.wallet_path).await? {
        if let (Some(name), Some(address)) = (row["coldkey"].as_str(), row["ss58"].as_str()) {
            ctx.output.name_address(address, name).map_err(failure)?;
        }
    }
    for row in &ctx.books.addresses {
        if let (Some(name), Some(address)) = (row["name"].as_str(), row["address"].as_str()) {
            ctx.output.name_address(address, name).map_err(failure)?;
        }
    }
    let address_names = ctx.output.address_names().map_err(failure)?;
    let owner_text = owner.to_string();
    let owner_text = match address_names.get(&owner_text) {
        Some(name) => format!("{name} ({owner_text})"),
        None => owner_text,
    };
    ctx.output
        .transfer_history(&format!("transfer history — {owner_text}"), &records)
        .map_err(failure)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Fixture {
        pages: Mutex<std::collections::VecDeque<Value>>,
        urls: Mutex<Vec<Url>>,
    }
    impl Transport for Fixture {
        fn get<'a>(
            &'a self,
            url: &'a Url,
        ) -> rtao::signer::BoxFuture<'a, Result<Vec<u8>, CliError>> {
            Box::pin(async move {
                self.urls.lock().map_err(failure)?.push(url.clone());
                let page = self
                    .pages
                    .lock()
                    .map_err(failure)?
                    .pop_front()
                    .ok_or_else(|| failure("unexpected page"))?;
                serde_json::to_vec(&page).map_err(failure)
            })
        }
    }
    #[tokio::test]
    async fn page_limit_upgrade_and_exact_indexer_records() -> Result<(), Box<dyn std::error::Error>>
    {
        let fixture = Fixture { pages: Mutex::new([
            json!({"results":[{"from_coldkey":"owner","to_coldkey":"other","amount":"18446744073709551615","from_netuid":0,"success":true}],"next":"http://api.taomarketcap.com/internal/v1/transactions/transfers/?page=2"}),
            json!({"results":[{"from_coldkey":"other","to_coldkey":"owner","amount":1,"to_netuid":3,"success":false},{"from_coldkey":"ignored"}]}),
        ].into()), urls: Mutex::new(Vec::new()) };
        let rows = fetch(&fixture, "owner", 2).await?;
        assert_eq!(rows.len(), 2);
        let urls = fixture.urls.lock().map_err(|e| e.to_string())?;
        assert_eq!(urls[0].query(), Some("coldkey=owner&limit=2"));
        assert_eq!(urls[1].scheme(), "https");
        assert_eq!(
            record("owner", &rows[0]),
            json!({"block_number":null,"extrinsic_idx":null,"timestamp":null,"amount_rao":"18446744073709551615","netuid":0,"from":"owner","to":"other","direction":"out","success":true,"tao_price_usd":null})
        );
        assert_eq!(record("owner", &rows[1])["direction"], "in");
        assert_eq!(
            record(
                "owner",
                &json!({"from_coldkey":"owner","to_coldkey":"owner","from_netuid":8})
            )["direction"],
            "self"
        );
        Ok(())
    }
    #[tokio::test]
    async fn refuses_foreign_pagination_and_cycles_without_following_them()
    -> Result<(), Box<dyn std::error::Error>> {
        for next in [
            "https://evil.example/steal",
            "https://api.taomarketcap.com/other",
            "https://user:password@api.taomarketcap.com/internal/v1/transactions/transfers/",
            "https://api.taomarketcap.com:444/internal/v1/transactions/transfers/",
        ] {
            let fixture = Fixture {
                pages: Mutex::new([json!({"results":[],"next":next})].into()),
                urls: Mutex::new(Vec::new()),
            };
            assert!(fetch(&fixture, "owner", 3).await.is_err());
            assert_eq!(fixture.urls.lock().map_err(|e| e.to_string())?.len(), 1);
        }
        let fixture = Fixture { pages: Mutex::new([json!({"results":[],"next":"https://api.taomarketcap.com/internal/v1/transactions/transfers/?coldkey=owner&limit=3"})].into()), urls: Mutex::new(Vec::new()) };
        assert!(fetch(&fixture, "owner", 3).await.is_err());
        assert_eq!(fixture.urls.lock().map_err(|e| e.to_string())?.len(), 1);
        assert!(fetch(&fixture, "owner", -1).await?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn malformed_pages_and_aggregate_budget_refuse_without_partial_output()
    -> Result<(), Box<dyn std::error::Error>> {
        for body in [
            json!([]),
            json!({"results":null}),
            json!({"results":[1]}),
            json!({"results":[],"next":true}),
        ] {
            let fixture = Fixture {
                pages: Mutex::new([body].into()),
                urls: Mutex::new(Vec::new()),
            };
            assert!(fetch(&fixture, "owner", 2).await.is_err());
        }
        struct Oversized;
        impl Transport for Oversized {
            fn get<'a>(
                &'a self,
                _: &'a Url,
            ) -> rtao::signer::BoxFuture<'a, Result<Vec<u8>, CliError>> {
                Box::pin(async { Ok(vec![b' '; MAX_BYTES + 1]) })
            }
        }
        let error = fetch(&Oversized, "owner", 1)
            .await
            .err()
            .ok_or("oversized body must fail")?;
        assert_eq!(error.message, "transfer history exceeds 16 MiB");
        Ok(())
    }
}

#[cfg(test)]
mod renderer_command_tests {
    use super::*;
    use crate::{
        application,
        books::Books,
        globals::Globals,
        output::{MemorySink, OutputOptions, Stream},
        prompt::LiveTerminal,
    };
    use std::sync::Mutex;

    struct Page(Value);
    impl Transport for Page {
        fn get<'a>(&'a self, _: &'a Url) -> rtao::signer::BoxFuture<'a, Result<Vec<u8>, CliError>> {
            Box::pin(async move { serde_json::to_vec(&self.0).map_err(failure) })
        }
    }

    #[tokio::test]
    async fn actual_history_command_projects_indexer_records_through_shared_output()
    -> Result<(), Box<dyn std::error::Error>> {
        let task_root = std::path::PathBuf::from(
            std::env::var_os("RTAO_TEMP_ROOT").ok_or("hosted tests require RTAO_TEMP_ROOT")?,
        );
        let wallets = task_root
            .join("tmp")
            .join(format!("history-renderer-no-files-{}", std::process::id()));
        if wallets.try_exists()? {
            return Err("fixture wallet path must be absent".into());
        }
        let owner = rtao::primitives::AccountId([7; 32]).to_string();
        let peer = rtao::primitives::AccountId([8; 32]).to_string();
        let parsed = application::command_tree([super::super::super::command()])
            .try_get_matches_from([
                "rtao",
                "--network",
                "finney",
                "--wallet-path",
                wallets.to_str().ok_or("wallet path")?,
                "wallet",
                "history",
                "--coldkey",
                &owner,
                "--limit",
                "2",
            ])?;
        let matches = parsed
            .subcommand()
            .ok_or("wallet")?
            .1
            .subcommand()
            .ok_or("history")?
            .1;
        let raw = vec![
            json!({"block_number":23,"extrinsic_idx":2,"timestamp":"2026-10-16T12:45:00Z","amount":"2000000000","from_coldkey":peer,"to_coldkey":owner,"to_netuid":0,"success":true,"tao_price_usd":"42"}),
            json!({"block_number":22,"extrinsic_idx":1,"timestamp":"not a date","amount":1_000_000_000u64,"from_coldkey":owner,"to_coldkey":peer,"from_netuid":0,"success":false}),
        ];
        let expected: Vec<_> = raw.iter().map(|row| record(&owner, row)).collect();
        for json_mode in [false, true] {
            let mut globals = Globals::resolve(&parsed, &Default::default(), &task_root, |_| None)?;
            globals.apply_with_env(matches, &[], |_| None)?;
            let mut ctx = AppContext::new(
                globals,
                Default::default(),
                Books::from_values(
                    json!([{"address":owner,"name":"my wallet"},{"address":peer,"name":"friend"}]),
                    json!([]),
                    json!([]),
                ),
                Arc::new(Mutex::new(Box::new(LiveTerminal))),
            );
            let sink = MemorySink::default();
            ctx.output = Output::new(
                OutputOptions {
                    json_mode,
                    quiet: true,
                    network: None,
                },
                Arc::new(sink.clone()),
            );
            run_with_transport(
                &mut ctx,
                matches,
                &mut PromptHistory::default(),
                Some(&Page(json!({"results":raw}))),
            )
            .await?;
            let text = sink.text(Stream::Stdout)?;
            if json_mode {
                assert_eq!(serde_json::from_str::<Value>(&text)?, json!(expected));
            } else {
                assert!(text.contains(&format!("transfer history — my wallet ({owner})")));
                assert!(text.contains(&format!("from friend ({peer})")));
                assert!(text.contains("2026-10-16 12:45"));
                assert!(text.contains("not a date"));
                assert!(text.contains("−τ1.000000000"));
                assert!(text.contains("✗ failed"));
                assert!(text.contains("2 transfers  ·  in τ2.000000000  ·  1 failed excluded"));
                assert!(!text.contains("amount (rao)"));
            }
            assert!(sink.bytes(Stream::Stderr)?.is_empty());
        }
        assert!(!wallets.try_exists()?);
        Ok(())
    }
}
