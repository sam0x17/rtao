#![no_main]
use libfuzzer_sys::fuzz_target;
use rtao_core::keyfile_json;
fuzz_target!(|bytes: &[u8]| {
    let nacl = keyfile_json::keyfile_data_is_encrypted_nacl(bytes);
    let ansible = keyfile_json::keyfile_data_is_encrypted_ansible(bytes);
    let legacy = keyfile_json::keyfile_data_is_encrypted_legacy(bytes);
    assert_eq!(
        keyfile_json::keyfile_data_is_encrypted(bytes),
        nacl || ansible || legacy
    );
    let _ = keyfile_json::keyfile_data_encryption_method(bytes);
});
