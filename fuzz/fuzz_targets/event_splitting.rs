#![no_main]

use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;
use rtao_client::Events;
use rtao_runtime::RuntimeHandle;

fuzz_target!(|bytes: &[u8]| {
    static RUNTIME: OnceLock<RuntimeHandle> = OnceLock::new();
    let runtime = RUNTIME.get_or_init(|| {
        let runtime = RuntimeHandle::parse(
            include_bytes!("../../vendor/metadata/finney/478.v15.scale"),
            478,
            1,
            42,
        )
        .expect("committed metadata must parse");
        let seed = Events::decode(
            runtime.clone(),
            include_bytes!("../seeds/event_splitting/new-account")
                .to_vec()
                .into(),
        )
        .expect("committed nonempty event seed must decode through the core");
        assert_eq!(seed.records().len(), 1);
        assert!(seed.records()[0].matches("System", "NewAccount"));
        runtime
    });
    if let Ok(events) = Events::decode(runtime.clone(), bytes.to_vec().into()) {
        assert_eq!(events.raw().len(), events.records().len());
        for (raw, record) in events.raw().iter().zip(events.records()) {
            assert_eq!(raw.pallet(), record.module_id);
            assert_eq!(raw.name(), record.event_id);
            assert_eq!(raw.pallet_index(), record.event_index);
            assert_eq!(raw.phase().name(), record.phase);
            assert_eq!(raw.phase().extrinsic_index(), record.extrinsic_idx);
            assert_eq!(raw.topics(), record.topics);
        }
    }
});
