#![no_main]
use libfuzzer_sys::fuzz_target;
use rtao_core::keyfile_json;
fuzz_target!(|bytes: &[u8]| {
    if bytes.len() > 64 * 1024 {
        return;
    }
    if let Ok(keypair) = keyfile_json::deserialize_keypair_from_keyfile_data(bytes) {
        let encoded = keyfile_json::serialized_keypair_to_keyfile_data(&keypair)
            .expect("decoded keypair serializes");
        let restored = keyfile_json::deserialize_keypair_from_keyfile_data(&encoded)
            .expect("serialized keypair round-trips");
        assert_eq!(keypair.public_key_bytes(), restored.public_key_bytes());
        assert_eq!(keypair.crypto_type(), restored.crypto_type());
    }
});
