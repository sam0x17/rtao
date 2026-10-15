#![no_main]

use libfuzzer_sys::fuzz_target;
use rtao_client::UntrustedUnsignedExtrinsic;
use rtao_primitives::H256;
use rtao_runtime::RuntimeHandle;
use rtao_signer::SigningContext;
use std::sync::OnceLock;

fuzz_target!(|data: &[u8]| {
    static TRUSTED: OnceLock<(RuntimeHandle, SigningContext)> = OnceLock::new();
    let (runtime, context) = TRUSTED.get_or_init(|| {
        let runtime = RuntimeHandle::parse(
            include_bytes!("../../vendor/metadata/finney/478.v15.scale"),
            478,
            1,
            42,
        )
        .expect("committed runtime must parse");
        let context = SigningContext {
            metadata_bytes: runtime.core().metadata_bytes.clone(),
            chain_info: rtao_core::digest::ChainInfo {
                spec_version: 478,
                spec_name: "node-subtensor".into(),
                base58_prefix: 42,
                decimals: 9,
                token_symbol: "TAO".into(),
            },
            transaction_version: 1,
            genesis_hash: [
                0x2f, 0x05, 0x55, 0xcc, 0x76, 0xfc, 0x28, 0x40, 0xa2, 0x5a, 0x6e, 0xa3, 0xb9, 0x63,
                0x71, 0x46, 0x80, 0x6f, 0x1f, 0x44, 0xb0, 0x90, 0xc1, 0x75, 0xff, 0xde, 0x2a, 0x7e,
                0x5a, 0xb3, 0x6c, 0x03,
            ],
        };
        (runtime, context)
    });
    // Every mutation enters the actual untrusted parser and authenticated
    // reconstruction. Inputs cannot select runtime, context or era anchor.
    if let Ok(untrusted) = UntrustedUnsignedExtrinsic::from_json(data)
        && let Ok(unsigned) = untrusted.validate(runtime, context, H256(context.genesis_hash), 100)
    {
        let exported = unsigned.to_json().expect("validated export must serialize");
        let encoded = serde_json::to_vec(&exported).expect("validated JSON serialization");
        let imported = UntrustedUnsignedExtrinsic::from_json(&encoded)
            .expect("validated export must parse")
            .validate(runtime, context, H256(context.genesis_hash), 100)
            .expect("validated export must reconstruct");
        assert_eq!(unsigned.call(), imported.call());
        assert_eq!(unsigned.payload(), imported.payload());
        assert_eq!(unsigned.payload_json(), imported.payload_json());
        assert_eq!(
            unsigned.included_in_extrinsic(),
            imported.included_in_extrinsic()
        );
        assert_eq!(
            unsigned.included_in_signed_data(),
            imported.included_in_signed_data()
        );
    }
});
