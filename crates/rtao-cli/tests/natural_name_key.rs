// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_natural_name_key.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Natural wallet-name contracts through the production comparator and listing.

use rtao_cli::commands::wallet::{list_records, natural_cmp};
use serde_json::{Value, json};
use std::{error::Error, fs, path::Path};

// Reuse the existing recorded-root guard; these fixtures create no processes,
// configuration references or dependencies outside their disposable directory.
#[path = "../../rtao-wallet/tests/support/mod.rs"]
mod support;
use support::TempRoot;

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn test_natural_name_key_orders_numbered_suffixes() {
    // pyt: unit/test_natural_name_key.py::test_natural_name_key_orders_numbered_suffixes
    let mut names = ["coldkey10", "coldkey2", "coldkey1", "coldkey11", "coldkey"];
    names.sort_by(|left, right| natural_cmp(left, right));
    assert_eq!(
        names,
        ["coldkey", "coldkey1", "coldkey2", "coldkey10", "coldkey11"]
    );
}

#[test]
fn test_natural_name_key_is_case_insensitive() {
    // pyt: unit/test_natural_name_key.py::test_natural_name_key_is_case_insensitive
    let mut names = ["Coldkey10", "coldkey2", "COLDKEY1"];
    names.sort_by(|left, right| natural_cmp(left, right));
    assert_eq!(names, ["COLDKEY1", "coldkey2", "Coldkey10"]);
}

#[test]
fn test_natural_name_key_compares_digit_and_text_chunks() {
    // pyt: unit/test_natural_name_key.py::test_natural_name_key_compares_digit_and_text_chunks
    let mut names = ["2wallet", "awallet", "wallet"];
    names.sort_by(|left, right| natural_cmp(left, right));
    assert_eq!(names, ["2wallet", "awallet", "wallet"]);
}

#[tokio::test]
async fn test_list_wallets_uses_natural_order() -> TestResult {
    // pyt: unit/test_natural_name_key.py::test_list_wallets_uses_natural_order
    let root = TempRoot::new();
    for name in ["coldkey10", "coldkey2", "coldkey1"] {
        let hotkeys = root.path().join(name).join("hotkeys");
        fs::create_dir_all(&hotkeys)?;
        for hotkey in ["hotkey10", "hotkey2", "hotkey1"] {
            fs::write(hotkeys.join(hotkey), b"{}")?;
        }
    }

    // Rust's public listing returns ordered records rather than a Python dict.
    // Project names without sorting them, preserving the production order.
    let listed = list_records(root.path()).await?;
    let names = listed
        .iter()
        .map(|record| record["coldkey"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [json!("coldkey1"), json!("coldkey2"), json!("coldkey10")]
    );
    for wallet in &listed {
        let hotkeys = wallet["hotkeys"].as_array().ok_or("hotkey list")?;
        let names = hotkeys
            .iter()
            .map(|hotkey| hotkey["name"].clone())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [json!("hotkey1"), json!("hotkey2"), json!("hotkey10")]
        );
    }
    Ok(())
}

fn write_pub(path: &Path, ss58: &str) -> TestResult {
    fs::write(
        path,
        serde_json::to_vec(&json!({"ss58Address": ss58, "cryptoType": 1}))?,
    )?;
    Ok(())
}

#[tokio::test]
async fn test_list_wallets_detailed_uses_natural_order() -> TestResult {
    // pyt: unit/test_natural_name_key.py::test_list_wallets_detailed_uses_natural_order
    let root = TempRoot::new();
    for (index, name) in ["coldkey10", "coldkey2", "coldkey1"].iter().enumerate() {
        let wallet = root.path().join(name);
        fs::create_dir_all(wallet.join("hotkeys"))?;
        write_pub(&wallet.join("coldkeypub.txt"), &format!("ck{}", index + 1))?;
        for (index, hotkey) in ["hotkey10", "hotkey2", "hotkey1"].iter().enumerate() {
            write_pub(
                &wallet.join("hotkeys").join(hotkey),
                &format!("{name}-hk{}", index + 1),
            )?;
        }
    }

    let detailed = list_records(root.path()).await?;
    // Native detailed records use coldkey/name fields and scheme labels.
    // Each expected record is independent of the production sorting function.
    assert_eq!(
        Value::Array(detailed),
        json!([
            {"coldkey":"coldkey1", "ss58":"ck3", "crypto_type":"sr25519", "hotkeys":[
                {"name":"hotkey1", "ss58":"coldkey1-hk3", "crypto_type":"sr25519"},
                {"name":"hotkey2", "ss58":"coldkey1-hk2", "crypto_type":"sr25519"},
                {"name":"hotkey10", "ss58":"coldkey1-hk1", "crypto_type":"sr25519"}
            ]},
            {"coldkey":"coldkey2", "ss58":"ck2", "crypto_type":"sr25519", "hotkeys":[
                {"name":"hotkey1", "ss58":"coldkey2-hk3", "crypto_type":"sr25519"},
                {"name":"hotkey2", "ss58":"coldkey2-hk2", "crypto_type":"sr25519"},
                {"name":"hotkey10", "ss58":"coldkey2-hk1", "crypto_type":"sr25519"}
            ]},
            {"coldkey":"coldkey10", "ss58":"ck1", "crypto_type":"sr25519", "hotkeys":[
                {"name":"hotkey1", "ss58":"coldkey10-hk3", "crypto_type":"sr25519"},
                {"name":"hotkey2", "ss58":"coldkey10-hk2", "crypto_type":"sr25519"},
                {"name":"hotkey10", "ss58":"coldkey10-hk1", "crypto_type":"sr25519"}
            ]}
        ])
    );
    Ok(())
}
