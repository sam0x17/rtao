// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/registration.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Host-verified, public-identity PoW registration.

pub use crate::generated::PowRegister;
use rtao_client::View;
use rtao_tx::{BuildCx, Built, Intent, IntentError, PreflightCx};

impl PowRegister {
    fn validated_seal(&self) -> Result<[u8; 32], IntentError> {
        let seal = self.seal()?;
        if self.netuid.0 == 0 {
            return Err(IntentError::validation(
                "PoW requires a non-root u16 netuid and a u64 work block",
            ));
        }
        Ok(seal)
    }

    fn seal(&self) -> Result<[u8; 32], IntentError> {
        // bytes.fromhex permits ASCII whitespace only between complete bytes.
        // removeprefix is case-sensitive and precedes decoding in Python.
        let input = self.work_hex.strip_prefix("0x").unwrap_or(&self.work_hex);
        let invalid = |position| {
            IntentError::validation(format!(
                "non-hexadecimal number found in fromhex() arg at position {position}"
            ))
        };
        if let Some(position) = input.chars().position(|ch| !ch.is_ascii()) {
            return Err(invalid(position));
        }
        let mut bytes = Vec::with_capacity(32);
        let mut chars = input.char_indices();
        while let Some((position, upper)) = chars.next() {
            if matches!(upper, ' ' | '\t' | '\n' | '\r' | '\u{b}' | '\u{c}') {
                continue;
            }
            let upper = upper.to_digit(16).ok_or_else(|| invalid(position))?;
            let (position, lower) = chars.next().ok_or_else(|| invalid(input.len()))?;
            let lower = lower.to_digit(16).ok_or_else(|| invalid(position))?;
            bytes.push(((upper << 4) | lower) as u8);
        }
        bytes
            .try_into()
            .map_err(|_| IntentError::validation("PoW requires a 32-byte seal and a u64 nonce"))
    }
}

impl Intent for PowRegister {
    fn summary(&self) -> String {
        let hotkey = self
            .hotkey_ss58
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "wallet hotkey".into());
        format!("PoW register {hotkey} on netuid {}", self.netuid.0)
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        let seal = self.validated_seal()?;
        #[cfg(feature = "pow")]
        {
            let work = crate::pow::RegistrationWork {
                block: self.work_block,
                nonce: self.nonce,
                seal,
            };
            let hotkey =
                cx.hotkey_address(self.hotkey_ss58.as_ref().map(|value| value.account()))?;
            let coldkey = cx.coldkey_address()?;
            let snapshot = cx.view.pin().await?;
            crate::pow::validate_registration(&snapshot, self.netuid, hotkey, coldkey, &work)
                .await
                .map_err(pow_error)?;
            Ok(Built::new(
                rtao_runtime::calls::subtensor_module::pow_register(
                    self.netuid,
                    self.work_block,
                    self.nonce,
                    work.seal,
                    hotkey,
                )
                .compose(cx.runtime)?,
            ))
        }
        #[cfg(not(feature = "pow"))]
        {
            let _ = (cx, seal);
            Err(IntentError::validation(
                "pow_register requires the rtao-intents `pow` feature (core host capability)",
            ))
        }
    }

    async fn effects<V: View + ?Sized>(
        &self,
        _cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        Ok(vec![
            self.summary(),
            "no TAO registration burn or upfront collateral purchase".into(),
            "direct submission with zero tip has no transaction fee".into(),
        ])
    }

    fn validate(&self) -> Result<(), IntentError> {
        self.validated_seal().map(|_| ())
    }

    fn semantic_intent(&self) -> &dyn rtao_tx::DynIntent {
        self
    }
}

#[cfg(feature = "pow")]
fn pow_error(error: crate::pow::PowError) -> IntentError {
    use crate::pow::PowError;
    let name = match &error {
        PowError::Client(_) => None,
        PowError::RootSubnet => Some("RegistrationNotPermittedOnRootSubnet"),
        PowError::SubnetNotExists => Some("SubnetNotExists"),
        PowError::Disabled => Some("SubNetRegistrationDisabled"),
        PowError::AlreadyRegistered => Some("HotKeyAlreadyRegisteredInSubNet"),
        PowError::NoNeuronIdAvailable => Some("NoNeuronIdAvailable"),
        PowError::ZeroRegistrationLimit | PowError::RegistrationLimitReached => {
            Some("TooManyRegistrationsThisBlock")
        }
        PowError::InvalidWorkBlock => Some("InvalidWorkBlock"),
        PowError::InvalidProof | PowError::InvalidSeal => Some("InvalidSeal"),
        PowError::InvalidDifficulty => Some("InvalidDifficulty"),
        _ => None,
    };
    match error {
        PowError::Client(error) => error.into(),
        error => IntentError::chain(error.to_string(), name.map(str::to_owned)),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]

    use super::*;
    #[cfg(feature = "pow")]
    use rtao_client::HeadProvider;
    use rtao_client::{BlockRef, Client, ClientConfig, Network};
    use rtao_primitives::{AccountId, NetUid};
    use rtao_signer::{KeyView, SignerError};
    use rtao_testkit::{FakeChain, FakeState};
    use rtao_tx::{IntentInterface, OriginView, SystemClock};
    use serde_json::json;

    const NETUID: NetUid = NetUid(1);
    const HOT: AccountId = AccountId([2; 32]);
    const COLD: AccountId = AccountId([3; 32]);

    // This public-only fixture cannot unlock or sign: it implements no signer
    // or key-source trait, and the build context exposes only these addresses.
    struct PublicKeys {
        hotkey: AccountId,
        coldkey: AccountId,
    }
    impl KeyView for PublicKeys {
        fn hotkey_address(&self) -> Result<AccountId, SignerError> {
            Ok(self.hotkey)
        }
        fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
            Ok(self.hotkey.0)
        }
        fn coldkey_address(&self) -> Result<AccountId, SignerError> {
            Ok(self.coldkey)
        }
    }
    const KEYS: PublicKeys = PublicKeys {
        hotkey: HOT,
        coldkey: COLD,
    };

    fn args() -> serde_json::Value {
        json!({"netuid":1,"work_block":100,"nonce":0,"work_hex":"00".repeat(32)})
    }

    fn intent() -> PowRegister {
        serde_json::from_value(args()).unwrap()
    }

    fn seed_admission(state: &mut FakeState, netuid: NetUid) {
        state
            .seed_typed("SubtensorModule", "NetworksAdded", (netuid,), true)
            .unwrap();
        state
            .seed_typed("SubtensorModule", "MaxAllowedUids", (netuid,), 16u16)
            .unwrap();
        state
            .seed_typed(
                "SubtensorModule",
                "NetworkRegistrationAllowed",
                (netuid,),
                false,
            )
            .unwrap();
        state
            .seed_typed(
                "SubtensorModule",
                "NetworkPowRegistrationAllowed",
                (netuid,),
                true,
            )
            .unwrap();
        for (item, value) in [
            ("Difficulty", 1),
            ("MinDifficulty", 1),
            ("MaxDifficulty", u64::MAX),
        ] {
            state
                .seed_typed("SubtensorModule", item, (netuid,), value)
                .unwrap();
        }
        state
            .seed_typed(
                "SubtensorModule",
                "MaxRegistrationsPerBlock",
                (netuid,),
                1u16,
            )
            .unwrap();
        state
            .seed_typed("SubtensorModule", "RegistrationsThisBlock", (netuid,), 0u16)
            .unwrap();
    }

    async fn fixture() -> (Client, FakeChain) {
        // Python's generic test defaults include Uids=0. Start with authentic
        // metadata and explicit admission state so an absent hotkey stays absent.
        let mut state = FakeState::new(FakeState::default().runtime);
        state.set_finalized(98).unwrap();
        seed_admission(&mut state, NETUID);
        let fake = FakeChain::from_state(state);
        let mut config =
            ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
        config.runtime_cache_dir = None;
        config.config_store = None;
        config.track_heads = false;
        let client = Client::from_rpc(fake.clone().into(), config).await.unwrap();
        (client, fake)
    }

    async fn build(
        client: &Client,
        intent: &PowRegister,
        origin: OriginView<'_>,
    ) -> Result<Built, IntentError> {
        let snapshot = client.at(BlockRef::Best).await.unwrap();
        intent
            .build(&BuildCx {
                view: &snapshot,
                origin,
                runtime: &snapshot.block().runtime,
                clock: &SystemClock,
                spec: PowRegister::SPEC,
            })
            .await
    }

    #[cfg(feature = "pow")]
    async fn proof(client: &Client, work_block: u64, nonce: u64) -> PowRegister {
        let hash = client.rpc().block_hash(work_block).await.unwrap().unwrap();
        let prefix = crate::pow::registration_prefix(NETUID, hash, HOT, COLD).unwrap();
        PowRegister {
            work_block,
            nonce,
            work_hex: hex::encode(rtao_core::pow::seal(&prefix, nonce).unwrap()),
            ..intent()
        }
    }

    #[cfg(feature = "pow")]
    fn named(error: IntentError, expected: &str) {
        let IntentError::Chain(error) = error else {
            panic!("expected named chain error {expected}, got {error}");
        };
        assert_eq!(error.name.as_deref(), Some(expected));
    }

    #[test]
    fn python_seal_syntax_and_exact_length() {
        for input in [
            "00".repeat(32),
            format!("0x{}", "aB".repeat(32)),
            format!("0x {}\t\n\r\u{b}\u{c}", "aB ".repeat(32)),
        ] {
            let mut intent = intent();
            intent.work_hex = input;
            assert!(intent.validate().is_ok());
        }
        for (input, diagnostic) in [
            (String::new(), "32-byte seal"),
            ("00".repeat(31), "32-byte seal"),
            ("00".repeat(33), "32-byte seal"),
            ("0X00".into(), "position 1"),
            ("0 0".into(), "position 1"),
            ("g0".into(), "position 0"),
            ("0".into(), "position 1"),
            ("ggé".into(), "position 2"),
            ("00\u{a0}00".into(), "position 2"),
        ] {
            let mut intent = intent();
            intent.work_hex = input;
            assert!(
                intent
                    .validate()
                    .unwrap_err()
                    .to_string()
                    .contains(diagnostic)
            );
        }
        let mut root = intent();
        root.netuid = NetUid::ROOT;
        assert!(
            root.validate()
                .unwrap_err()
                .to_string()
                .contains("non-root u16")
        );
    }

    #[test]
    fn typed_widths_defaults_json_and_public_summary() {
        let mut value = args();
        value["nonce"] = json!(u64::MAX);
        value["work_block"] = json!(u64::MAX);
        value["netuid"] = json!(u16::MAX);
        value["hotkey_ss58"] = json!(HOT.to_ss58(2));
        let intent: PowRegister = serde_json::from_value(value).unwrap();
        assert_eq!(intent.nonce, u64::MAX);
        assert_eq!(intent.mining_workers, 4);
        assert_eq!(intent.mining_timeout, serde_json::Number::from(300));
        assert_eq!(intent.mining_backend, "auto");
        assert_eq!(intent.mining_device_ids, None);
        assert_eq!(intent.to_json()["hotkey_ss58"], HOT.to_ss58(2));
        assert_eq!(
            intent.summary(),
            format!("PoW register {} on netuid 65535", HOT.to_ss58(2))
        );
        assert_eq!(intent.spend(), rtao_primitives::Spend::None);
        assert_eq!(intent.touches_netuids().as_slice(), &[NetUid(u16::MAX)]);
        let mut serialized = intent.to_json();
        serialized.as_object_mut().unwrap().remove("op");
        assert_eq!(
            serde_json::from_value::<PowRegister>(serialized).unwrap(),
            intent
        );
        for (field, invalid) in [
            ("netuid", json!(-1)),
            ("netuid", json!(65536)),
            ("nonce", json!(-1)),
            ("nonce", json!(18_446_744_073_709_551_616u128)),
            ("work_block", json!(-1)),
            ("work_block", json!(18_446_744_073_709_551_616u128)),
            ("work_block", json!(1.5)),
        ] {
            let mut value = args();
            value[field] = invalid;
            assert!(serde_json::from_value::<PowRegister>(value).is_err());
        }
        assert_eq!(
            self::intent().summary(),
            "PoW register wallet hotkey on netuid 1"
        );
    }

    #[cfg(not(feature = "pow"))]
    #[tokio::test]
    async fn registration_profile_reports_required_host_capability() {
        let (client, fake) = fixture().await;
        let error = build(&client, &intent(), OriginView::Keys(&KEYS))
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("`pow` feature (core host capability)")
        );
        assert!(fake.requests().iter().all(|request| {
            !request.method.starts_with("author_") && request.method != "state_queryStorageAt"
        }));
        client.close().await;
    }

    #[cfg(feature = "pow")]
    #[tokio::test]
    async fn authenticated_runtime_call_uses_exact_work_and_only_public_identities() {
        let (client, fake) = fixture().await;
        let mut intent = proof(&client, 100, u64::MAX).await;
        intent.hotkey_ss58 = Some(HOT.to_ss58(2).parse().unwrap());
        let other_attached_hotkey = PublicKeys {
            hotkey: AccountId([99; 32]),
            coldkey: COLD,
        };
        let built = build(&client, &intent, OriginView::Keys(&other_attached_hotkey))
            .await
            .unwrap();
        let snapshot = client.at(BlockRef::Best).await.unwrap();
        // Independently supply Python's semantic arguments to the core's live
        // metadata composer, rather than invoking the typed constructor again.
        let expected = snapshot
            .block()
            .runtime
            .core()
            .compose_call(
                "SubtensorModule",
                "pow_register",
                &rtao_core::codec::Value::record(vec![
                    ("netuid".into(), rtao_core::codec::Value::Int(1)),
                    ("work_block".into(), rtao_core::codec::Value::Int(100)),
                    (
                        "nonce".into(),
                        rtao_core::codec::Value::Int(i128::from(u64::MAX)),
                    ),
                    (
                        "work".into(),
                        rtao_core::codec::Value::Bytes(intent.seal().unwrap().to_vec()),
                    ),
                    (
                        "hotkey".into(),
                        rtao_core::codec::Value::Str(HOT.to_string()),
                    ),
                ]),
            )
            .unwrap();
        assert_eq!(built.call.as_bytes(), expected);
        assert_eq!(built.call.as_bytes().get(..2), Some([7, 152].as_slice()));
        assert!(built.extras.is_empty());
        assert!(
            fake.requests()
                .iter()
                .all(|request| !request.method.starts_with("author_"))
        );
        let pinned = format!("0x{}", hex::encode(snapshot.block().hash.0));
        let queries = fake
            .requests()
            .into_iter()
            .filter(|request| request.method == "state_queryStorageAt")
            .collect::<Vec<_>>();
        assert_eq!(queries.len(), 2);
        assert!(queries.iter().all(|request| request.params[1] == pinned));
        let preflight = intent
            .effects(&PreflightCx {
                view: &snapshot,
                runtime: &snapshot.block().runtime,
                clock: &SystemClock,
                spec: PowRegister::SPEC,
                client: &client,
                dispatch_origin: COLD,
                fee_payer: COLD,
                call: &built.call,
            })
            .await
            .unwrap();
        assert_eq!(
            preflight,
            [
                intent.summary(),
                "no TAO registration burn or upfront collateral purchase".into(),
                "direct submission with zero tip has no transaction fee".into()
            ]
        );
        client.close().await;
    }

    #[cfg(feature = "pow")]
    #[tokio::test]
    async fn every_challenge_identity_is_verified() {
        for changed in ["hotkey", "coldkey", "netuid", "nonce", "seal"] {
            let (client, fake) = fixture().await;
            let mut intent = proof(&client, 100, 0).await;
            let mut keys = PublicKeys {
                hotkey: HOT,
                coldkey: COLD,
            };
            match changed {
                "hotkey" => keys.hotkey = AccountId([4; 32]),
                "coldkey" => keys.coldkey = AccountId([5; 32]),
                "netuid" => {
                    intent.netuid = NetUid(2);
                    fake.mutate_state(|state| seed_admission(state, NetUid(2)));
                }
                "nonce" => intent.nonce = 1,
                _ => intent.work_hex = "00".repeat(32),
            }
            named(
                build(&client, &intent, OriginView::Keys(&keys))
                    .await
                    .unwrap_err(),
                "InvalidSeal",
            );
            client.close().await;
        }
    }

    #[cfg(feature = "pow")]
    #[tokio::test]
    async fn maximum_non_root_netuid_uses_its_own_authenticated_challenge() {
        let (client, fake) = fixture().await;
        let netuid = NetUid(u16::MAX);
        fake.mutate_state(|state| seed_admission(state, netuid));
        let hash = client.rpc().block_hash(100).await.unwrap().unwrap();
        let prefix = crate::pow::registration_prefix(netuid, hash, HOT, COLD).unwrap();
        let intent = PowRegister {
            netuid,
            work_hex: hex::encode(rtao_core::pow::seal(&prefix, 0).unwrap()),
            ..intent()
        };
        let built = build(&client, &intent, OriginView::Keys(&KEYS))
            .await
            .unwrap();
        assert_eq!(
            built.call.as_bytes().get(..4),
            Some([7, 152, 255, 255].as_slice())
        );
        client.close().await;
    }

    #[cfg(feature = "pow")]
    #[tokio::test]
    async fn root_disabled_uid_zero_and_capacity_reject_before_submission() {
        for (case, expected) in [
            ("absent", "SubnetNotExists"),
            ("disabled", "SubNetRegistrationDisabled"),
            ("uid_zero", "HotKeyAlreadyRegisteredInSubNet"),
            ("no_uids", "NoNeuronIdAvailable"),
            ("zero_limit", "TooManyRegistrationsThisBlock"),
            ("used_limit", "TooManyRegistrationsThisBlock"),
        ] {
            let (client, fake) = fixture().await;
            let intent = proof(&client, 100, 0).await;
            fake.mutate_state(|state| match case {
                "absent" => state
                    .seed_typed("SubtensorModule", "NetworksAdded", (NETUID,), false)
                    .unwrap(),
                "disabled" => state
                    .seed_typed(
                        "SubtensorModule",
                        "NetworkPowRegistrationAllowed",
                        (NETUID,),
                        false,
                    )
                    .unwrap(),
                "uid_zero" => state
                    .seed_typed("SubtensorModule", "Uids", (NETUID, HOT), 0u16)
                    .unwrap(),
                "no_uids" => state
                    .seed_typed("SubtensorModule", "MaxAllowedUids", (NETUID,), 0u16)
                    .unwrap(),
                "zero_limit" => state
                    .seed_typed(
                        "SubtensorModule",
                        "MaxRegistrationsPerBlock",
                        (NETUID,),
                        0u16,
                    )
                    .unwrap(),
                _ => state
                    .seed_typed("SubtensorModule", "RegistrationsThisBlock", (NETUID,), 1u16)
                    .unwrap(),
            });
            named(
                build(&client, &intent, OriginView::Keys(&KEYS))
                    .await
                    .unwrap_err(),
                expected,
            );
            assert!(
                fake.requests()
                    .iter()
                    .all(|request| !request.method.starts_with("author_"))
            );
            client.close().await;
        }
        let (client, fake) = fixture().await;
        let mut intent = intent();
        intent.netuid = NetUid::ROOT;
        assert!(
            build(&client, &intent, OriginView::Keys(&KEYS))
                .await
                .unwrap_err()
                .to_string()
                .contains("non-root u16")
        );
        assert!(
            fake.requests()
                .iter()
                .all(|request| request.method != "state_queryStorageAt")
        );
        client.close().await;
    }

    #[cfg(feature = "pow")]
    #[tokio::test]
    async fn published_age_four_is_usable_five_and_future_are_expired() {
        let (client, _) = fixture().await;
        for block in [96, 100] {
            let intent = proof(&client, block, 0).await;
            assert!(
                build(&client, &intent, OriginView::Keys(&KEYS))
                    .await
                    .is_ok()
            );
        }
        for block in [95, 101, u64::MAX] {
            let mut intent = proof(&client, 100, 0).await;
            intent.work_block = block;
            named(
                build(&client, &intent, OriginView::Keys(&KEYS))
                    .await
                    .unwrap_err(),
                "InvalidWorkBlock",
            );
        }
        client.close().await;
    }

    #[cfg(feature = "pow")]
    #[tokio::test]
    async fn reused_work_and_missing_or_zero_challenge_hash_are_rejected() {
        for case in ["reused", "missing", "zero"] {
            let (client, fake) = fixture().await;
            let intent = proof(&client, 100, 0).await;
            fake.mutate_state(|state| {
                if case == "reused" {
                    state
                        .seed_typed(
                            "SubtensorModule",
                            "LastPowRegistrationBlock",
                            (HOT,),
                            100u64,
                        )
                        .unwrap();
                } else {
                    state.set_handler("chain_getBlockHash", move |state, request| {
                        if request.params.get(0).and_then(serde_json::Value::as_u64) == Some(100) {
                            rtao_testkit::Reply::result(if case == "missing" {
                                json!(null)
                            } else {
                                json!(format!("0x{}", "00".repeat(32)))
                            })
                        } else {
                            state.dispatch(request)
                        }
                    });
                }
            });
            named(
                build(&client, &intent, OriginView::Keys(&KEYS))
                    .await
                    .unwrap_err(),
                "InvalidWorkBlock",
            );
            client.close().await;
        }
    }

    #[cfg(feature = "pow")]
    #[tokio::test]
    async fn reorged_work_hash_cannot_be_submitted() {
        let (client, fake) = fixture().await;
        let intent = proof(&client, 100, 0).await;
        fake.mutate_state(|state| {
            let parent = state.block_hash(99).unwrap();
            let fork = state.fork(&parent).unwrap();
            state.set_best(&fork).unwrap();
        });
        named(
            build(&client, &intent, OriginView::Keys(&KEYS))
                .await
                .unwrap_err(),
            "InvalidSeal",
        );
        client.close().await;
    }

    #[cfg(feature = "pow")]
    #[tokio::test]
    async fn current_effective_difficulty_rejects_previously_valid_work() {
        for item in ["Difficulty", "MinDifficulty"] {
            let (client, fake) = fixture().await;
            let hash = client.rpc().block_hash(100).await.unwrap().unwrap();
            let prefix = crate::pow::registration_prefix(NETUID, hash, HOT, COLD).unwrap();
            let nonce = (0..128)
                .find(|nonce| {
                    rtao_core::pow::mine_cpu(&prefix, 2, *nonce, 1)
                        .unwrap()
                        .is_none()
                })
                .unwrap();
            let intent = proof(&client, 100, nonce).await;
            fake.mutate_state(|state| {
                state
                    .seed_typed("SubtensorModule", item, (NETUID,), 2u64)
                    .unwrap()
            });
            named(
                build(&client, &intent, OriginView::Keys(&KEYS))
                    .await
                    .unwrap_err(),
                "InvalidDifficulty",
            );
            client.close().await;
        }
    }

    #[cfg(feature = "pow")]
    #[tokio::test]
    async fn explicit_hotkey_and_bare_coldkey_work_without_wallet_or_secrets() {
        let (client, _) = fixture().await;
        let mut intent = proof(&client, 100, 0).await;
        assert!(
            build(&client, &intent, OriginView::Account(COLD))
                .await
                .unwrap_err()
                .to_string()
                .contains("no hotkey to default to")
        );
        intent.hotkey_ss58 = Some(HOT.into());
        assert!(
            build(&client, &intent, OriginView::Account(COLD))
                .await
                .is_ok()
        );
        client.close().await;
    }
}
