// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_null_weights.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Exact full-row Null input through the production CLI file reader.

use rtao_cli::{application, commands::weights};
use serde_json::{Map, Value, json};

// Reuse the recorded-root guard; the fixture creates one JSON file, with no
// processes, configuration references or dependencies outside its directory.
#[path = "../../rtao-wallet/tests/support/mod.rs"]
mod support;
use support::TempRoot;

// pyt: sdk/python/tests/unit/test_null_weights.py::test_cli_full_row_file
#[test]
fn cli_full_row_file_preserves_the_complete_original_mapping()
-> Result<(), Box<dyn std::error::Error>> {
    let root = TempRoot::new();
    let path = root.path().join("weights.json");
    let mut data = (0..2_500)
        .map(|uid| (uid.to_string(), json!(1)))
        .collect::<Map<String, Value>>();
    data.insert("0".into(), json!(65_534));
    std::fs::write(&path, serde_json::to_vec(&data)?)?;
    let parsed = application::command_tree([weights::command()]).try_get_matches_from([
        std::ffi::OsString::from("rtao"),
        "weights".into(),
        "set".into(),
        "--netuid".into(),
        "1".into(),
        "--weights-file".into(),
        path.into_os_string(),
        "--raw-u16".into(),
    ])?;
    let group = parsed.subcommand().ok_or("weights group")?.1;
    let request = weights::request(group)?;
    // An omitted JSON field replaces the Python helper's None UID component.
    assert!(request.args.get("uids").is_none());
    assert_eq!(request.args["weights"], Value::Object(data));
    assert_eq!(request.args["raw_u16"], json!(true));
    Ok(())
}
