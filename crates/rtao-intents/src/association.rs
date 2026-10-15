// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/association.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Public hotkey ownership and EVM ownership-proof associations.

use rtao_client::View;
use rtao_core::codec::Value;
use rtao_primitives::IntoCoreValue;
use rtao_runtime::{Call, calls::subtensor_module};
use rtao_tx::{BuildCx, Built, Intent, IntentError};

pub use crate::{AssociateEvmKey, AssociateHotkey};

impl Intent for AssociateHotkey {
    fn summary(&self) -> String {
        let hotkey = self
            .hotkey_ss58
            .as_ref()
            .map_or("the wallet hotkey", rtao_primitives::Ss58Address::as_str);
        format!("associate hotkey {hotkey} with the coldkey")
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        crate::generated::passthrough::associate_hotkey(self, cx).await
    }
}

impl Intent for AssociateEvmKey {
    fn summary(&self) -> String {
        format!(
            "associate EVM key {} with the hotkey on netuid {}",
            self.evm_key, self.netuid
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        // Python evaluates bytes.fromhex before compose, then the native
        // encoder validates H160 before the signature's fixed-array length.
        // Keep the decoded signature dynamic until the checked descriptor is
        // composed so malformed fields retain that order of diagnostics.
        let signature = signature_bytes(&self.signature)?;
        let call = Call::new(
            &subtensor_module::ASSOCIATE_EVM_KEY,
            Value::record(vec![
                ("netuid".into(), self.netuid.into_core_value()),
                ("evm_key".into(), Value::Str(self.evm_key.clone())),
                ("block_number".into(), self.block_number.into_core_value()),
                ("signature".into(), Value::Bytes(signature)),
            ]),
            Vec::new(),
        );
        Ok(Built::new(call.compose(cx.runtime)?))
    }
}

// CPython v3.12.11 Objects/bytesobject.c, _PyBytes_FromHex:
// git blob dae84127a7df4b7aa2bb328285b733a9896bc01d.
// removeprefix("0x") is case-sensitive; whitespace is permitted between
// complete bytes, and the first non-ASCII code point takes diagnostic priority.
fn signature_bytes(input: &str) -> Result<Vec<u8>, IntentError> {
    let input = input.strip_prefix("0x").unwrap_or(input);
    let invalid = |position| {
        IntentError::validation(format!(
            "non-hexadecimal number found in fromhex() arg at position {position}"
        ))
    };
    if let Some(position) = input.chars().position(|character| !character.is_ascii()) {
        return Err(invalid(position));
    }
    let input = input.as_bytes();
    let mut bytes = Vec::with_capacity(input.len() / 2);
    let mut offset = 0;
    while offset < input.len() {
        while input
            .get(offset)
            .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c))
        {
            offset += 1;
        }
        if offset == input.len() {
            break;
        }
        let upper = nibble(input[offset]).ok_or_else(|| invalid(offset))?;
        offset += 1;
        let lower = input
            .get(offset)
            .copied()
            .and_then(nibble)
            .ok_or_else(|| invalid(offset))?;
        offset += 1;
        bytes.push((upper << 4) | lower);
    }
    Ok(bytes)
}

fn nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use rtao_client::{BlockRef, Client, ClientConfig, DynView, Network, Snapshot};
    use rtao_primitives::{AccountId, NetUid, Spend};
    use rtao_runtime::{CallBytes, RuntimeHandle};
    use rtao_signer::{KeyView, SignerError};
    use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
    use rtao_tx::{DynIntent, IntentInterface, OriginView, PreflightCx, SystemClock};
    use serde_json::json;

    const OWNER: AccountId = AccountId([7; 32]);
    const HOTKEY: AccountId = AccountId([8; 32]);
    const EVM_ADDRESS: &str = "0x00112233445566778899aAbBcCdDeEfF00112233";
    const EVM_BYTES: [u8; 20] = [
        0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee,
        0xff, 0x00, 0x11, 0x22, 0x33,
    ];

    struct PublicKeys;
    impl KeyView for PublicKeys {
        fn hotkey_address(&self) -> Result<AccountId, SignerError> {
            Ok(HOTKEY)
        }
        fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
            Ok(HOTKEY.0)
        }
        fn coldkey_address(&self) -> Result<AccountId, SignerError> {
            Ok(OWNER)
        }
    }

    async fn fixture() -> anyhow::Result<(FakeChain, Client, Snapshot)> {
        let chain = FakeChain::from_state(FakeState::try_default()?);
        let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
        config.track_heads = false;
        config.runtime_cache_dir = None;
        config.config_store = None;
        let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
        let snapshot = client.at(BlockRef::Finalized).await?;
        Ok((chain, client, snapshot))
    }

    fn decode(call: &CallBytes, runtime: &RuntimeHandle) -> anyhow::Result<serde_json::Value> {
        let decoded = runtime
            .core()
            .decode_spec(&rtao_core::runtime::TypeSpec::Call, call.as_bytes(), true)
            .map_err(rtao_core::CoreError::from)?;
        Ok(rtao_core::codec::value::to_corpus_json(&decoded))
    }

    fn evm_intent(signature: String) -> AssociateEvmKey {
        AssociateEvmKey {
            netuid: NetUid(513),
            evm_key: EVM_ADDRESS.into(),
            block_number: 0x0807_0605_0403_0201,
            signature,
        }
    }

    #[tokio::test]
    async fn association_hotkey_keeps_defaults_explicit_priority_and_ss58_spelling()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let requests = chain.requests().len();
        let default = crate::AnyIntent::from_tool("associate_hotkey", json!({}))?;
        assert_eq!(default.to_json()["hotkey_ss58"], serde_json::Value::Null);
        let intent = AssociateHotkey { hotkey_ss58: None };
        assert_eq!(
            intent.summary(),
            "associate hotkey the wallet hotkey with the coldkey"
        );
        let built = intent
            .build(&BuildCx {
                view: &snapshot,
                origin: OriginView::Keys(&PublicKeys),
                runtime,
                clock: &SystemClock,
                spec: AssociateHotkey::SPEC,
            })
            .await?;
        let mut expected = vec![7, 91];
        expected.extend_from_slice(&[8; 32]);
        assert_eq!(built.call.as_bytes(), expected);
        assert_eq!(CallBytes::decode(&expected, runtime)?, built.call);
        assert!(built.extras.is_empty());
        assert_eq!(
            decode(&built.call, runtime)?["call_function"],
            "try_associate_hotkey"
        );
        assert_eq!(
            decode(&built.call, runtime)?["call_args"][0]["value"],
            HOTKEY.to_string()
        );

        // A bare coldkey signer cannot substitute its own account for a hotkey.
        assert_eq!(
            intent
                .build(&BuildCx {
                    view: &snapshot,
                    origin: OriginView::Account(OWNER),
                    runtime,
                    clock: &SystemClock,
                    spec: AssociateHotkey::SPEC,
                })
                .await
                .expect_err("missing wallet hotkey must refuse")
                .to_string(),
            "associate_hotkey: this signer carries no hotkey to default to; pass the hotkey address explicitly or sign with a Wallet"
        );
        for prefix in [0, 2, 42, 110] {
            let spelling = AccountId::ZERO.to_ss58(prefix);
            let intent = AssociateHotkey {
                hotkey_ss58: Some(spelling.parse()?),
            };
            let built = intent
                .build(&BuildCx {
                    view: &snapshot,
                    origin: OriginView::Keys(&PublicKeys),
                    runtime,
                    clock: &SystemClock,
                    spec: AssociateHotkey::SPEC,
                })
                .await?;
            let mut expected = vec![7, 91];
            expected.extend_from_slice(&[0; 32]);
            assert_eq!(built.call.as_bytes(), expected);
            assert_eq!(CallBytes::decode(&expected, runtime)?, built.call);
            assert_eq!(intent.to_json()["hotkey_ss58"], spelling);
            assert_eq!(
                intent.summary(),
                format!("associate hotkey {spelling} with the coldkey")
            );
        }
        assert!(
            crate::AnyIntent::from_tool(
                "associate_hotkey",
                json!({"hotkey_ss58":AccountId::ZERO.to_ss58(1000)})
            )
            .is_err()
        );
        assert_eq!(chain.requests().len(), requests);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn association_evm_encodes_fixed_fields_without_changing_or_verifying_the_proof()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let requests = chain.requests().len();
        let proof: Vec<u8> = (0..65).collect();
        let plain = hex::encode(&proof);
        for signature in [
            plain.clone(),
            format!("0x{plain}"),
            format!("0x{}", plain.to_ascii_uppercase()),
            format!(
                "0x\t{}\n",
                proof
                    .iter()
                    .map(|byte| format!("{byte:02X}"))
                    .collect::<Vec<_>>()
                    .join(" \r\x0b\x0c")
            ),
        ] {
            let intent = evm_intent(signature.clone());
            let built = intent
                .build(&BuildCx {
                    view: &snapshot,
                    origin: OriginView::Account(HOTKEY),
                    runtime,
                    clock: &SystemClock,
                    spec: AssociateEvmKey::SPEC,
                })
                .await?;
            // Fixed independent SCALE layout: indices, u16, H160, u64, [u8;65].
            let mut expected = vec![7, 93, 1, 2];
            expected.extend_from_slice(&EVM_BYTES);
            expected.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
            expected.extend_from_slice(&proof);
            assert_eq!(built.call.as_bytes(), expected);
            assert_eq!(CallBytes::decode(&expected, runtime)?, built.call);
            assert!(built.extras.is_empty());
            let decoded = decode(&built.call, runtime)?;
            assert_eq!(decoded["call_module"], "SubtensorModule");
            assert_eq!(decoded["call_function"], "associate_evm_key");
            let arguments = decoded["call_args"]
                .as_array()
                .expect("decoded call has an argument array");
            let named_values: Vec<_> = arguments
                .iter()
                .map(|argument| json!({"name":argument["name"],"value":argument["value"]}))
                .collect();
            assert_eq!(
                json!(named_values),
                json!([
                    {"name":"netuid","value":513},
                    {"name":"evm_key","value":"0x00112233445566778899aabbccddeeff00112233"},
                    {"name":"block_number","value":0x0807_0605_0403_0201u64},
                    {"name":"signature","value":format!("0x{plain}")},
                ])
            );
            assert_eq!(intent.to_json()["evm_key"], EVM_ADDRESS);
            assert_eq!(intent.to_json()["signature"], signature);
            assert_eq!(
                crate::AnyIntent::from_tool("associate_evm_key", intent.to_json())?.to_json(),
                intent.to_json()
            );
        }
        assert_eq!(chain.requests().len(), requests);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[test]
    fn association_signature_preserves_python_fromhex_whitespace_and_errors() -> anyhow::Result<()>
    {
        for (input, expected) in [
            ("", vec![]),
            ("0x", vec![]),
            (" \t\n\r\x0b\x0c", vec![]),
            ("0x00aBFf", vec![0, 0xab, 0xff]),
            ("0x\t01 02\n03\r04\x0b05\x0c06", vec![1, 2, 3, 4, 5, 6]),
        ] {
            assert_eq!(signature_bytes(input)?, expected, "{input:?}");
        }
        for (input, position) in [
            ("0", 1),
            ("0x0", 1),
            ("0X00", 1),
            ("0x0x00", 1),
            ("0 0", 1),
            ("00 0g", 4),
            (" 0x00", 2),
            ("é", 0),
            ("ggé", 2),
            ("0x00😀", 2),
            ("00\u{a0}11", 2),
            ("00\x1c11", 2),
        ] {
            assert_eq!(
                signature_bytes(input)
                    .expect_err("malformed signature must refuse")
                    .to_string(),
                format!("non-hexadecimal number found in fromhex() arg at position {position}"),
                "{input:?}"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn association_evm_validates_hex_then_address_then_fixed_signature_length()
    -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let requests = chain.requests().len();
        for (address, signature, message) in [
            (
                "0x11",
                "0X00".into(),
                "non-hexadecimal number found in fromhex() arg at position 1",
            ),
            (
                "0x11",
                "0x22".into(),
                "codec error: expected 20 bytes, got 1",
            ),
            (
                EVM_ADDRESS,
                "0x22".into(),
                "codec error: expected 65 bytes, got 1",
            ),
            (
                EVM_ADDRESS,
                String::new(),
                "codec error: expected 65 bytes, got 0",
            ),
            (
                EVM_ADDRESS,
                "22".repeat(66),
                "codec error: expected 65 bytes, got 66",
            ),
        ] {
            let mut intent = evm_intent(signature);
            intent.evm_key = address.into();
            // The Python dataclass has no constructor/preflight validation.
            intent.validate()?;
            assert_eq!(
                intent
                    .build(&BuildCx {
                        view: &snapshot,
                        origin: OriginView::Account(HOTKEY),
                        runtime,
                        clock: &SystemClock,
                        spec: AssociateEvmKey::SPEC,
                    })
                    .await
                    .expect_err("malformed association must refuse")
                    .to_string(),
                message
            );
        }
        // H160 is passed through to the same native codec as Python. Its
        // byte-carrier contract also accepts twenty UTF-8 bytes and repeated
        // lowercase prefixes; imposing an extra address parser changes parity.
        for address in ["a".repeat(20), format!("0x0x{}", "ab".repeat(20))] {
            let mut intent = evm_intent("22".repeat(65));
            intent.evm_key = address;
            assert!(
                intent
                    .build(&BuildCx {
                        view: &snapshot,
                        origin: OriginView::Account(HOTKEY),
                        runtime,
                        clock: &SystemClock,
                        spec: AssociateEvmKey::SPEC,
                    })
                    .await
                    .is_ok()
            );
        }
        assert_eq!(chain.requests().len(), requests);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn association_preflight_and_policy_keep_inherited_defaults() -> anyhow::Result<()> {
        let (chain, client, snapshot) = fixture().await?;
        let runtime = &snapshot.block().runtime;
        let requests = chain.requests().len();
        let hotkey = AssociateHotkey { hotkey_ss58: None };
        let evm = evm_intent("22".repeat(65));
        for (intent, summary, signer, touched) in [
            (
                &hotkey as &dyn DynIntent,
                "associate hotkey the wallet hotkey with the coldkey".to_owned(),
                "coldkey",
                vec![],
            ),
            (
                &evm as &dyn DynIntent,
                format!("associate EVM key {EVM_ADDRESS} with the hotkey on netuid 513"),
                "hotkey",
                vec![NetUid(513)],
            ),
        ] {
            let built = intent
                .build_dyn(&BuildCx {
                    view: &snapshot as &dyn DynView,
                    origin: OriginView::Keys(&PublicKeys),
                    runtime,
                    clock: &SystemClock,
                    spec: intent.spec(),
                })
                .await?;
            let preview = intent
                .preflight_dyn(&PreflightCx {
                    view: &snapshot as &dyn DynView,
                    runtime,
                    clock: &SystemClock,
                    spec: intent.spec(),
                    client: &client,
                    dispatch_origin: OWNER,
                    fee_payer: OWNER,
                    call: &built.call,
                })
                .await?;
            assert_eq!(intent.summary_dyn(), summary);
            assert_eq!(preview.effects, vec![summary]);
            assert!(preview.warnings.is_empty());
            assert!(preview.blocks.is_empty());
            assert!(preview.required_free.is_none());
            assert!(preview.available_free.is_none());
            assert!(preview.estimated_fee.is_none());
            assert!(preview.facts.is_empty());
            assert_eq!(intent.spend_dyn(), Spend::None);
            assert_eq!(intent.touches_netuids_dyn().as_slice(), touched);
            assert!(!intent.affects_all_subnets_dyn());
            assert_eq!(intent.spec().signer, signer);
            assert_eq!(intent.spec().origin, "signed");
            assert!(intent.spec().verify.is_none());
            assert!(intent.spec().all_amount_fields.is_empty());
        }
        assert_eq!(chain.requests().len(), requests);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    #[test]
    fn association_tool_inputs_enforce_native_ranges_and_account_validation() {
        for args in [
            json!({"hotkey_ss58":"invalid"}),
            json!({"hotkey_ss58":""}),
            json!({"hotkey_ss58":7}),
            json!({"unexpected":true}),
            json!([]),
        ] {
            assert!(crate::AnyIntent::from_tool("associate_hotkey", args).is_err());
        }
        assert!(
            crate::AnyIntent::from_tool("associate_hotkey", json!({"hotkey_ss58":null})).is_ok()
        );
        let valid = json!({"netuid":65535,"evm_key":EVM_ADDRESS,"block_number":u64::MAX,"signature":"22".repeat(65)});
        assert!(crate::AnyIntent::from_tool("associate_evm_key", valid.clone()).is_ok());
        for field in ["netuid", "evm_key", "block_number", "signature"] {
            let mut missing = valid.clone();
            missing
                .as_object_mut()
                .expect("fixture object")
                .remove(field);
            assert!(crate::AnyIntent::from_tool("associate_evm_key", missing).is_err());
            let mut null = valid.clone();
            null[field] = serde_json::Value::Null;
            assert!(crate::AnyIntent::from_tool("associate_evm_key", null).is_err());
        }
        for (field, value) in [
            ("netuid", json!(-1)),
            ("netuid", json!(65536)),
            ("block_number", json!(-1)),
            (
                "block_number",
                serde_json::from_str("18446744073709551616")
                    .expect("fixture is an arbitrary-precision JSON number"),
            ),
            ("evm_key", json!(22)),
            ("signature", json!(vec![0; 65])),
            ("unexpected", json!(true)),
        ] {
            let mut invalid = valid.clone();
            invalid[field] = value;
            assert!(
                crate::AnyIntent::from_tool("associate_evm_key", invalid).is_err(),
                "{field}"
            );
        }
    }
}
