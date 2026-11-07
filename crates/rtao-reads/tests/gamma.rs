// Derived from RaoFoundation/subtensor sdk/python/bittensor/reads/gamma.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_gamma.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
//! Gamma deposit, mint quote, TAO/USD price and service reads.

#![allow(clippy::unwrap_used)] // Assertions must fail when fake-chain fixtures or responses are invalid.

use rtao_client::{BlockRef, Client, ClientConfig, Network};
use rtao_primitives::{AccountId, NetUid};
use rtao_reads::{ReadError, reads::gamma};
use rtao_testkit::rpc::RpcClient;
use rtao_testkit::{FakeChain, FakeState};
use serde_json::{Value, json};

const RAO: u64 = 1_000_000_000;

fn account(byte: u8) -> AccountId {
    AccountId([byte; 32])
}

fn ss58(byte: u8) -> String {
    account(byte).to_string()
}

async fn client(state: FakeState) -> (Client, FakeChain) {
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let chain = FakeChain::from_state(state);
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config)
        .await
        .unwrap();
    (client, chain)
}

async fn head(client: &Client) -> u64 {
    client.at(BlockRef::Best).await.unwrap().block().number
}

fn gamma_disabled() -> Value {
    let error = rtao_runtime::errors::MODULE_ERRORS
        .iter()
        .find(|error| error.pallet == "SubtensorModule" && error.name == "GammaDisabled")
        .unwrap();
    json!({"Module": {"index": error.pallet_index, "error": [error.error_index, 0, 0, 0]}})
}

// pyt: unit/test_gamma.py::TestGammaReads::test_amounts_are_exact_and_the_feed_has_an_age
#[tokio::test]
async fn amounts_are_exact_and_the_feed_has_an_age() {
    let mut state = FakeState::default();
    state
        .seed_runtime(
            "GammaRuntimeApi",
            "sim_mint_gamma",
            json!({"Ok": [RAO, RAO, (1u64 << 53) + 1]}),
        )
        .unwrap();
    let (client, chain) = client(state).await;
    let block = head(&client).await;
    chain.mutate_state(|state| {
        state
            .seed_python(
                "SubtensorModule",
                "TaoUsdPrice",
                &[],
                json!({"accepted": 312_450_000_000u64, "feed": 312_500_000_000u64, "fed_at": block - 7}),
            )
            .unwrap();
    });
    let quote = gamma::quote_mint_gamma(&client, NetUid(1), 1.0)
        .await
        .unwrap();
    assert_eq!(quote.gamma, "9007199.254740993");
    assert_eq!(quote.alpha_burned.rao, RAO);
    assert_eq!(quote.alpha_burned.netuid, NetUid(1));
    assert_eq!(quote.tao_value.netuid, NetUid::ROOT);
    let price = gamma::tao_usd_price(&client).await.unwrap();
    assert_eq!(price.accepted_usd, Some(312.5));
    assert_eq!(price.blocks_since_last_feed, Some(7));
    assert_eq!(price.feed_usd, Some(312.5));
    assert_eq!(price.fed_at, Some(block - 7));
    assert_eq!(price.oracle, None);
    client.close().await;
}

// pyt: unit/test_gamma.py::TestGammaReads::test_the_accepted_price_rises_by_the_step_up_to_the_feed
#[test]
fn the_accepted_price_rises_by_the_step_up_to_the_feed() {
    for (accepted, feed, blocks, price) in [
        (312_450_000_000u128, 312_500_000_000u128, 0, 312.45),
        (312_450_000_000, 312_500_000_000, 1, 312.470_621_7),
        (1, 312_500_000_000, 2, 3e-9),
    ] {
        assert_eq!(
            gamma::accepted_price(accepted, feed, blocks) as f64 / 1e9,
            price,
            "{accepted} {blocks}"
        );
    }
}

#[tokio::test]
async fn an_unfed_price_reads_as_absent() {
    let (client, _) = client(FakeState::default()).await;
    let price = gamma::tao_usd_price(&client).await.unwrap();
    assert_eq!(
        serde_json::to_value(&price).unwrap(),
        json!({"oracle": null, "accepted_usd": null, "feed_usd": null, "fed_at": null, "blocks_since_last_feed": null})
    );
    client.close().await;
}

// pyt: unit/test_gamma.py::TestGammaReads::test_quotes_ask_in_chain_units
// pyt: unit/test_gamma.py::TestGammaReads::test_a_refused_quote_raises_the_chain_error
#[tokio::test]
async fn quotes_ask_in_chain_units_and_a_refusal_is_the_chain_error() {
    let mut state = FakeState::default();
    for method in ["sim_mint_gamma", "sim_mint_gamma_with_stake"] {
        state
            .seed_runtime("GammaRuntimeApi", method, json!({"Ok": [1, 2, 3]}))
            .unwrap();
    }
    let (client, chain) = client(state).await;
    let quote = gamma::quote_mint_gamma(&client, NetUid(3), 1.5)
        .await
        .unwrap();
    assert_eq!(
        (
            quote.alpha_burned.rao,
            quote.tao_value.rao,
            quote.gamma.as_str()
        ),
        (1, 2, "0.000000003")
    );
    gamma::quote_mint_gamma_with_stake(&client, NetUid(3), 2.0)
        .await
        .unwrap();
    let calls: Vec<Value> = chain
        .requests()
        .iter()
        .filter(|request| request.method == "state_call")
        .map(|request| request.params[0].clone())
        .collect();
    assert!(calls.contains(&json!("GammaRuntimeApi_sim_mint_gamma")));
    assert!(calls.contains(&json!("GammaRuntimeApi_sim_mint_gamma_with_stake")));
    // Arguments are chain units: 1.5 TAO is 1.5e9 rao, 2 alpha 2e9.
    let arguments: Vec<String> = chain
        .requests()
        .iter()
        .filter(|request| request.method == "state_call")
        .filter_map(|request| request.params[1].as_str().map(str::to_owned))
        .collect();
    let encode = |amount: u64| {
        let mut bytes = 3u16.to_le_bytes().to_vec();
        bytes.extend(amount.to_le_bytes());
        format!("0x{}", hex::encode(bytes))
    };
    assert!(arguments.contains(&encode(1_500_000_000)), "{arguments:?}");
    assert!(arguments.contains(&encode(2 * RAO)), "{arguments:?}");
    for method in ["sim_mint_gamma", "sim_mint_gamma_with_stake"] {
        chain.mutate_state(|state| {
            state
                .seed_runtime("GammaRuntimeApi", method, json!({"Err": gamma_disabled()}))
                .unwrap();
        });
    }
    for result in [
        gamma::quote_mint_gamma(&client, NetUid(1), 1.0).await,
        gamma::quote_mint_gamma_with_stake(&client, NetUid(1), 1.0).await,
    ] {
        assert!(
            matches!(&result, Err(ReadError::Chain { .. })),
            "{result:?}"
        );
        if let Err(ReadError::Chain { name, message }) = result {
            assert_eq!(name.as_deref(), Some("GammaDisabled"));
            assert_eq!(
                message,
                "Gamma minting is disabled network-wide or on this subnet."
            );
        }
    }
    client.close().await;
}

// pyt: unit/test_gamma.py::TestGammaReads::test_deposit_reads_are_exact_and_tell_no_cap_from_a_paused_one
#[tokio::test]
async fn deposit_reads_are_exact_and_tell_no_cap_from_a_paused_one() {
    let info = |cap: Option<u64>| {
        let daily_spend_cap = cap.map(|cap| json!({"cap": cap, "bucket": [0, 0]}));
        json!({
            "netuid": 1,
            "hotkey": ss58(2),
            "deposit": {"coldkey": ss58(1), "spender": ss58(3), "balance": 3 * RAO + 1, "daily_spend_cap": daily_spend_cap},
            "spendable": RAO / 2,
        })
    };
    let mut state = FakeState::default();
    state
        .seed_runtime("GammaRuntimeApi", "get_gamma_deposit", info(Some(0)))
        .unwrap();
    state
        .seed_runtime(
            "GammaRuntimeApi",
            "get_gamma_deposits_for_coldkey",
            json!([info(None)]),
        )
        .unwrap();
    let (client, chain) = client(state).await;
    let deposit = gamma::gamma_deposit(&client, NetUid(1), account(2))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(&deposit).unwrap(),
        json!({
            "netuid": 1,
            "hotkey": ss58(2),
            "coldkey": ss58(1),
            "spender": ss58(3),
            "balance_gamma": "3.000000001",
            "daily_spend_cap_gamma": "0",
            "spendable_gamma": "0.5",
        })
    );
    let listed = gamma::gamma_deposits(&client, account(1)).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].daily_spend_cap_gamma, None);
    chain.mutate_state(|state| {
        state
            .seed_runtime("GammaRuntimeApi", "get_gamma_deposit", Value::Null)
            .unwrap();
    });
    assert_eq!(
        gamma::gamma_deposit(&client, NetUid(1), account(2))
            .await
            .unwrap(),
        None
    );
    client.close().await;
}

fn services_state(owner_of_owner_hotkey: u8) -> FakeState {
    let mut state = FakeState::default();
    let spoof = "https://a.example\u{1b}[1A\u{1b}[2K";
    for (hotkey, url, spender) in [
        (10u8, "https://a.example", 1u8),
        (11, spoof, 1),
        (12, "https://\u{202e}example", 3),
        // The literal text "0x41", seeded as its bytes (a 0x string seeds hex).
        (13, "0x30783431", 3),
    ] {
        state
            .seed_python(
                "SubtensorModule",
                "Services",
                &[json!(1), json!(ss58(hotkey))],
                json!({"url": url, "spender": ss58(spender)}),
            )
            .unwrap();
    }
    state
        .seed_python(
            "SubtensorModule",
            "SubnetOwnerHotkey",
            &[json!(1)],
            json!(ss58(10)),
        )
        .unwrap();
    state
        .seed_python(
            "SubtensorModule",
            "SubnetOwner",
            &[json!(1)],
            json!(ss58(1)),
        )
        .unwrap();
    state
        .seed_python(
            "SubtensorModule",
            "Owner",
            &[json!(ss58(10))],
            json!(ss58(owner_of_owner_hotkey)),
        )
        .unwrap();
    state
}

// pyt: unit/test_gamma.py::TestServicesRead::test_urls_with_control_or_format_characters_read_as_hex
// pyt: unit/test_gamma.py::TestServicesRead::test_an_owner_hotkey_another_coldkey_owns_marks_no_owner
#[tokio::test]
async fn services_read_unprintable_urls_as_hex_and_mark_the_owned_owner_hotkey() {
    let (owned, _) = client(services_state(1)).await;
    let services = gamma::services(&owned, NetUid(1)).await.unwrap();
    assert_eq!(
        serde_json::to_value(&services[&account(10)]).unwrap(),
        json!({"url": "https://a.example", "spender": ss58(1), "owner": true})
    );
    let spoof = "https://a.example\u{1b}[1A\u{1b}[2K";
    assert_eq!(
        services[&account(11)].url,
        format!("0x{}", hex::encode(spoof))
    );
    assert_eq!(services[&account(11)].spender, account(1));
    assert!(!services[&account(11)].owner);
    assert!(services[&account(12)].url.starts_with("0x"));
    assert_eq!(services[&account(13)].url, "0x41");
    owned.close().await;

    let (transferred, _) = client(services_state(3)).await;
    let services = gamma::services(&transferred, NetUid(1)).await.unwrap();
    assert!(!services[&account(10)].owner);
    transferred.close().await;
}
