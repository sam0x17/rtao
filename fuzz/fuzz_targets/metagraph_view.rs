#![no_main]

use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;
use rtao_metagraph::MetagraphView;
use rtao_runtime::RuntimeHandle;

fuzz_target!(|bytes: &[u8]| {
    static RUNTIME: OnceLock<RuntimeHandle> = OnceLock::new();
    let runtime = RUNTIME.get_or_init(|| {
        RuntimeHandle::parse(
            include_bytes!("../../vendor/metadata/finney/478.v15.scale"),
            478,
            1,
            42,
        )
        .expect("committed metadata must parse")
    });
    let _result = MetagraphView::parse(bytes, runtime);
});
