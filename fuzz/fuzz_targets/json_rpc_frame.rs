#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|bytes: &[u8]| {
    rtao_rpc::fuzz_json_rpc_frame(bytes).expect("offline parser probe must initialize");
});
