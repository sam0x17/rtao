//! Error behavior checked against the pinned Python source extraction.

use std::{collections::BTreeMap, error::Error};

use rtao_catalog::ErrorCode;
use rtao_client::{ClientError, PoolRejection};
use rtao_primitives::UnitError;
use rtao_rpc::RpcError;
use rtao_tx::{ChainError, IntentError, PolicyError, TxError, classify};
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn Error>>;
const RESULT: &str = "sdk/python/bittensor/result.py";
const ERROR_MAP: &str = "sdk/python/bittensor/error_map.py";

struct Fixtures {
    tables: Value,
    catalog: Value,
    settings: Value,
    descriptions: Value,
}

impl Fixtures {
    fn read() -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            tables: serde_json::from_str(include_str!(
                "../../../vendor/upstream/extracted/named_tables.json"
            ))?,
            catalog: serde_json::from_str(include_str!(
                "../../../vendor/upstream/catalog/errors.json"
            ))?,
            settings: serde_json::from_str(include_str!(
                "../../../vendor/upstream/extracted/settings.json"
            ))?,
            descriptions: serde_json::from_str(include_str!(
                "../../../vendor/upstream/extracted/error_descriptions.json"
            ))?,
        })
    }

    fn dict(&self, path: &str, name: &str) -> Result<&[Value], Box<dyn Error>> {
        self.tables["tables"][path]["values"][name]["value"]["dict"]
            .as_array()
            .map(Vec::as_slice)
            .ok_or_else(|| format!("missing extracted dictionary {path}:{name}").into())
    }

    fn string_dict(&self, path: &str, name: &str) -> Result<BTreeMap<&str, &str>, Box<dyn Error>> {
        self.dict(path, name)?
            .iter()
            .map(|row| Ok((text(&row[0])?, text(&row[1])?)))
            .collect()
    }

    fn docs_base(&self) -> Result<&str, Box<dyn Error>> {
        text(&self.settings["values"]["DOCS_URL"]["value"])
    }
}

fn text(value: &Value) -> Result<&str, Box<dyn Error>> {
    value
        .as_str()
        .ok_or_else(|| "expected extracted string".into())
}

fn code(value: &Value) -> Result<ErrorCode, Box<dyn Error>> {
    ErrorCode::from_code(text(value)?).ok_or_else(|| "unrecognized extracted error code".into())
}

#[test]
fn all_392_named_errors_have_exact_classification_description_remediation_and_json() -> TestResult {
    let fixture = Fixtures::read()?;
    let remediation = fixture.string_dict(RESULT, "REMEDIATION")?;
    let overrides = fixture.string_dict(RESULT, "_NAME_HELP_OVERRIDES")?;
    let explanations = fixture.string_dict(RESULT, "EXPLANATIONS")?;
    let names = fixture.dict(ERROR_MAP, "NAME_TO_CODE")?;
    assert_eq!(names.len(), 392);
    for row in names {
        let name = text(&row[0])?;
        let token = text(&row[1])?;
        let expected_description =
            text(&fixture.descriptions["descriptions"][name]["description"])?;
        assert_eq!(
            expected_description,
            text(&fixture.catalog["chain_errors"][name]["description"])?
        );
        let error = ChainError::new("insufficient invalid transaction", Some(name.into()));
        assert_eq!(error.code, code(&row[1])?, "{name}");
        assert_eq!(error.description(), Some(expected_description), "{name}");
        let expected_help = overrides.get(name).copied().unwrap_or(
            "the node rejected the extrinsic before inclusion; check the detail line above",
        );
        assert_eq!(error.remediation(), expected_help, "{name}");
        assert_eq!(error.explanation(), explanations[token], "{name}");
        let url = format!("{}/errors/chain/{name}", fixture.docs_base()?);
        assert_eq!(error.docs_url().as_deref(), Some(url.as_str()), "{name}");
        assert_eq!(
            error.to_json(),
            json!({
                "message":"insufficient invalid transaction", "name":name, "code":token,
                "remediation":expected_help, "description":expected_description, "docs_url":url,
            }),
            "{name}"
        );
        let plain = ChainError::new("irrelevant", Some(name.into()));
        assert_eq!(
            plain.remediation(),
            overrides.get(name).copied().unwrap_or(remediation[token]),
            "{name}"
        );
    }
    Ok(())
}

#[test]
fn all_dispatch_names_respect_module_precedence_and_semantic_docs_urls() -> TestResult {
    let fixture = Fixtures::read()?;
    let chain = fixture.catalog["chain_errors"]
        .as_object()
        .ok_or("missing chain error catalog")?;
    let remediation = fixture.string_dict(RESULT, "REMEDIATION")?;
    let overrides = fixture.string_dict(RESULT, "_NAME_HELP_OVERRIDES")?;
    let explanations = fixture.string_dict(RESULT, "EXPLANATIONS")?;
    let rows = fixture.dict(ERROR_MAP, "DISPATCH_ERRORS")?;
    assert_eq!(rows.len(), 52);
    for row in rows {
        let name = text(&row[0])?;
        let dispatch_token = &row[1]["tuple"][0];
        let module = chain.get(name);
        let expected_code = module.map_or(dispatch_token, |info| &info["code"]);
        let token = text(expected_code)?;
        let expected_description = module.map_or(&row[1]["tuple"][1], |_| {
            &fixture.descriptions["descriptions"][name]["description"]
        });
        let error = ChainError::new("irrelevant", Some(name.into()));
        assert_eq!(error.code, code(expected_code)?, "{name}");
        assert_eq!(
            error.description(),
            Some(text(expected_description)?),
            "{name}"
        );
        assert_eq!(
            error.remediation(),
            overrides.get(name).copied().unwrap_or(remediation[token]),
            "{name}"
        );
        assert_eq!(error.explanation(), explanations[token], "{name}");
        let expected_url = if module.is_some() {
            Some(format!("{}/errors/chain/{name}", fixture.docs_base()?))
        } else if token != "unknown" {
            Some(format!(
                "{}/errors/{}",
                fixture.docs_base()?,
                token.replace('_', "-")
            ))
        } else {
            None
        };
        assert_eq!(error.docs_url(), expected_url, "{name}");
        let payload = error.to_json();
        assert_eq!(payload["description"], *expected_description, "{name}");
        let expected_docs = expected_url.map(Value::String);
        assert_eq!(payload.get("docs_url"), expected_docs.as_ref(), "{name}");
    }
    Ok(())
}

#[test]
fn every_semantic_code_retains_an_explicit_override_and_exact_explanation() -> TestResult {
    let fixture = Fixtures::read()?;
    let remediation = fixture.string_dict(RESULT, "REMEDIATION")?;
    let explanations = fixture.string_dict(RESULT, "EXPLANATIONS")?;
    assert_eq!(remediation.len(), 18);
    for (token, help) in remediation {
        let semantic = ErrorCode::from_code(token).ok_or("unknown semantic token")?;
        let error = ChainError::with_code("arbitrary", None, semantic);
        assert_eq!(error.code, semantic);
        assert_eq!(error.remediation(), help);
        assert_eq!(error.explanation(), explanations[token]);
        assert_eq!(error.description(), None);
        let expected_url = (semantic != ErrorCode::Unknown).then(|| {
            format!(
                "https://www.bittensor.com/docs/errors/{}",
                token.replace('_', "-")
            )
        });
        assert_eq!(error.docs_url(), expected_url);
        let mut expected =
            json!({"message":"arbitrary", "name":null, "code":token, "remediation":help});
        if let Some(url) = expected_url {
            expected["docs_url"] = json!(url);
        }
        assert_eq!(error.to_json(), expected);
    }
    Ok(())
}

#[test]
fn classification_and_remediation_apply_the_exact_precedence_gates() -> TestResult {
    let fixture = Fixtures::read()?;
    let needles =
        fixture.tables["tables"][RESULT]["values"]["_SUBSTRING_FALLBACK"]["value"]["tuple"]
            .as_array()
            .ok_or("missing fallback needles")?;
    for row in needles {
        let needle = text(&row["tuple"][0])?;
        let expected = code(&row["tuple"][1])?;
        assert_eq!(classify(&needle.to_uppercase(), None), expected, "{needle}");
        assert_eq!(classify(needle, Some("")), expected, "{needle}");
        assert_eq!(
            classify(needle, Some("Unrecognized")),
            ErrorCode::Unknown,
            "{needle}"
        );
    }
    assert_eq!(
        classify("insufficient balance", Some("SlippageTooHigh")),
        ErrorCode::InsufficientLiquidity
    );
    assert_eq!(
        classify("insufficient balance", Some("NewRateLimitExceeded")),
        ErrorCode::RateLimited
    );
    assert_eq!(
        classify("insufficient balance", Some("NewRateLimitWindow")),
        ErrorCode::RateLimited
    );
    assert_eq!(
        classify("insufficient balance", Some("newratelimit")),
        ErrorCode::Unknown
    );
    assert_eq!(
        classify("priority is too low; insufficient", None),
        ErrorCode::InsufficientBalance
    );
    assert_eq!(classify("", None), ErrorCode::Unknown);
    let named = ChainError::new("INVALID TRANSACTION", Some("SlippageTooHigh".into()));
    let overrides = fixture.string_dict(RESULT, "_NAME_HELP_OVERRIDES")?;
    assert_eq!(named.remediation(), overrides["SlippageTooHigh"]);
    let unnamed = ChainError::new("INVALID TRANSACTION", None);
    assert_eq!(
        unnamed.remediation(),
        "the node rejected the extrinsic before inclusion; check the detail line above"
    );
    let unknown_named = ChainError::new("INVALID TRANSACTION", Some("Unrecognized".into()));
    assert_eq!(unknown_named.code, ErrorCode::Unknown);
    assert_eq!(unknown_named.remediation(), unnamed.remediation());
    assert_eq!(unknown_named.docs_url(), None);
    assert_eq!(unknown_named.description(), None);
    Ok(())
}

#[test]
fn pool_watch_standard_and_unknown_diagnostics_use_python_messages() -> TestResult {
    let fixture = Fixtures::read()?;
    let dispatch: BTreeMap<_, _> = fixture
        .dict(ERROR_MAP, "DISPATCH_ERRORS")?
        .iter()
        .map(|row| Ok((text(&row[0])?, text(&row[1]["tuple"][1])?)))
        .collect::<Result<_, Box<dyn Error>>>()?;
    for rejection in [
        PoolRejection::Future,
        PoolRejection::Stale,
        PoolRejection::PriorityTooLow,
        PoolRejection::AlreadyImported,
        PoolRejection::InvalidTransaction("Payment".into()),
        PoolRejection::InvalidTransaction("BadProof".into()),
    ] {
        let name = rejection.name().ok_or("missing standard rejection name")?;
        let error = ChainError::from_pool(&rejection);
        assert_eq!(error.name.as_deref(), Some(name));
        assert_eq!(error.message, dispatch[name], "{name}");
    }
    let standards = fixture.tables["tables"][ERROR_MAP]["values"]["INVALID_TRANSACTION_ERRORS"]
        ["value"]["tuple"]
        .as_array()
        .ok_or("standard validity errors absent")?;
    assert_eq!(standards.len(), 16);
    for row in standards {
        let needle = text(&row["tuple"][0])?;
        let name = text(&row["tuple"][1])?;
        let rejection = PoolRejection::from_rpc(1010, &needle.to_uppercase(), None);
        let error = ChainError::from_pool(&rejection);
        assert_eq!(error.name.as_deref(), Some(name), "{needle}");
        assert_eq!(error.message, dispatch[name], "{name}");
    }
    for (status, name) in [
        ("DROPPED", "Dropped"),
        ("Usurped", "Usurped"),
        ("invalid", "Invalid"),
        ("Retracted", "Retracted"),
        ("finalitytimeout", "FinalityTimeout"),
    ] {
        let error =
            ChainError::from_pool(&PoolRejection::Watch(format!("Extrinsic {status}: {{}}")));
        assert_eq!(error.name.as_deref(), Some(name));
        assert_eq!(error.message, dispatch[name], "{name}");
    }
    for rejection in [
        PoolRejection::Unknown("insufficient free funds".into()),
        PoolRejection::TemporarilyBanned,
    ] {
        let error = ChainError::from_pool(&rejection);
        assert_eq!(error.name, None);
        assert_eq!(error.message, rejection.to_string());
        assert_eq!(error.code, classify(&rejection.to_string(), None));
    }
    // Python's generated module-docs dictionary replaces earlier names and
    // takes precedence over the system-dispatch description.
    assert_eq!(
        ChainError::from_pool(&PoolRejection::InvalidTransaction(
            "InvalidSignature".into()
        ))
        .message,
        "The provided signature does not match the order payload and signer."
    );
    assert_eq!(
        ChainError::from_pool(&PoolRejection::InvalidTransaction(
            "InsufficientBalance".into()
        ))
        .message,
        "The caller does not have enough balance for the operation."
    );
    assert_eq!(
        ChainError::from_pool(&PoolRejection::InvalidTransaction("Unclassified".into())).message,
        "Unclassified"
    );
    Ok(())
}

#[test]
fn typed_error_conversion_keeps_transport_and_validation_boundaries() {
    let closed = TxError::from(ClientError::Rpc(RpcError::Closed));
    assert!(matches!(closed, TxError::ConnectionNotReady));
    let transport = TxError::from(IntentError::Client(ClientError::Rpc(RpcError::Transport(
        "socket refused".into(),
    ))));
    assert!(matches!(transport, TxError::Rpc(RpcError::Transport(_))));
    assert_eq!(transport.to_string(), "socket refused");
    let unit = TxError::from(IntentError::UnitMismatch(UnitError::Overflow));
    assert!(matches!(unit, TxError::UnitMismatch(UnitError::Overflow)));
    assert_eq!(unit.code(), ErrorCode::UnitMismatch);
    let validation = IntentError::validation("exact Python constructor message");
    assert_eq!(validation.to_string(), "exact Python constructor message");
    assert_eq!(validation.code(), ErrorCode::InvalidArgument);
    let policy = PolicyError {
        violations: vec!["first violation".into(), "second violation".into()],
    };
    assert_eq!(policy.to_string(), "first violation; second violation");
    assert_eq!(policy.code(), ErrorCode::PolicyViolation);
}

#[test]
fn gateway_policy_conversion_retains_retry_and_provider_metadata() -> TestResult {
    let payload = json!({
        "code": -32004, "message": "Too many requests",
        "data": {"retry_after": "17", "policy": "quota", "reason": "minute limit"},
    });
    let refusal = rtao_rpc::policy::classify_rpc_error(&payload)
        .ok_or("policy refusal was not recognized")?;
    let converted = TxError::from(IntentError::Client(ClientError::Rpc(RpcError::Policy(
        Box::new(refusal.clone()),
    ))));
    let TxError::RpcPolicy(actual) = converted else {
        return Err("gateway policy became an untyped chain error".into());
    };
    assert_eq!(*actual, refusal);
    assert_eq!(actual.retry_after.as_deref(), Some("17"));
    assert_eq!(actual.policy.as_deref(), Some("quota"));
    assert_eq!(actual.reason.as_deref(), Some("minute limit"));
    Ok(())
}

#[test]
fn all_custom_byte_json_results_match_the_authenticated_python_tables() -> TestResult {
    let fixture = Fixtures::read()?;
    let custom_names: BTreeMap<_, _> = fixture
        .dict(ERROR_MAP, "CUSTOM_TRANSACTION_ERRORS")?
        .iter()
        .map(|row| Ok((row[0].as_u64().ok_or("custom byte absent")?, text(&row[1])?)))
        .collect::<Result<_, Box<dyn Error>>>()?;
    let mut module_messages = BTreeMap::new();
    for row in fixture.dict("sdk/python/bittensor/_generated/errors.py", "ERRORS")? {
        let fields = &row[1]["fields"];
        let docs = text(&fields["docs"])?;
        if !docs.is_empty() {
            module_messages.insert(text(&fields["name"])?, docs);
        }
    }
    let dispatch: BTreeMap<_, _> = fixture
        .dict(ERROR_MAP, "DISPATCH_ERRORS")?
        .iter()
        .map(|row| Ok((text(&row[0])?, &row[1]["tuple"])))
        .collect::<Result<_, Box<dyn Error>>>()?;
    let remediation = fixture.string_dict(RESULT, "REMEDIATION")?;
    let overrides = fixture.string_dict(RESULT, "_NAME_HELP_OVERRIDES")?;
    let chain = fixture.catalog["chain_errors"]
        .as_object()
        .ok_or("missing chain error catalog")?;
    for custom in 0..=u8::MAX {
        let data = json!({"Custom": custom});
        let error = ChainError::from_pool(&PoolRejection::from_rpc(
            1010,
            "Invalid Transaction",
            Some(&data),
        ));
        let expected = if let Some(name) = custom_names.get(&u64::from(custom)) {
            let module = chain.get(*name);
            let dispatch_info = dispatch.get(name);
            let token = if let Some(info) = module {
                text(&info["code"])?
            } else {
                text(&dispatch_info.ok_or("custom error classification absent")?[0])?
            };
            let message = module_messages.get(name).copied().map_or_else(
                || dispatch_info.map_or(Ok(*name), |info| text(&info[1])),
                Ok,
            )?;
            let description = module.map_or_else(
                || dispatch_info.map_or(Ok(None), |info| text(&info[1]).map(Some)),
                |_| text(&fixture.descriptions["descriptions"][*name]["description"]).map(Some),
            )?;
            let help = overrides.get(name).copied().unwrap_or_else(|| {
                if message.to_lowercase().contains("invalid transaction") {
                    "the node rejected the extrinsic before inclusion; check the detail line above"
                } else {
                    remediation[token]
                }
            });
            let url = if module.is_some() {
                Some(format!("{}/errors/chain/{name}", fixture.docs_base()?))
            } else if token != "unknown" {
                Some(format!(
                    "{}/errors/{}",
                    fixture.docs_base()?,
                    token.replace('_', "-")
                ))
            } else {
                None
            };
            let mut expected =
                json!({"message":message,"name":name,"code":token,"remediation":help});
            if let Some(description) = description.filter(|text| !text.is_empty()) {
                expected["description"] = json!(description);
            }
            if let Some(url) = url {
                expected["docs_url"] = json!(url);
            }
            expected
        } else {
            json!({
                "message":format!("Invalid Transaction: {{'Custom': {custom}}}"), "name":null,
                "code":"invalid_argument",
                "remediation":"the node rejected the extrinsic before inclusion; check the detail line above",
                "docs_url":format!("{}/errors/invalid-argument", fixture.docs_base()?),
            })
        };
        assert_eq!(error.to_json(), expected, "custom {custom}");
    }
    Ok(())
}
