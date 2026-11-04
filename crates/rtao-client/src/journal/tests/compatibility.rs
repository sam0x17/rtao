//! Format compatibility range and compaction (D-120).
//!
//! `fixtures/schema3-before-d110.json` was written through the public journal
//! API by the release before D-120 (snapshot-2026-11-03.2) against the
//! FakeChain with the fixed test key used throughout these tests. It holds
//! `logical-op` (an attempt that expired unexecuted, then a finalized one),
//! `pending-op` (one unresolved signed attempt), `queued-op` (no attempt) and
//! an application checkpoint.
use super::*;

const FIXTURE: &[u8] = include_bytes!("fixtures/schema3-before-d110.json");

fn fixture_journal(f: &Fixture) -> PathBuf {
    let path = f.0.join("journal.json");
    std::fs::write(&path, FIXTURE).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    path
}
fn raw(path: &std::path::Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}
fn write_raw(path: &std::path::Path, value: &Value) -> Vec<u8> {
    let bytes = serde_json::to_vec(value).unwrap();
    std::fs::write(path, &bytes).unwrap();
    bytes
}
fn logical_intent(image: &JournalImage) -> OperationIntent {
    image
        .operations
        .get("logical-op")
        .map(|r| r.intent.clone())
        .or_else(|| image.compacted.get("logical-op").map(|c| c.intent.clone()))
        .unwrap()
}

#[test]
fn schema3_written_before_d110_reads_and_stays_readable_by_that_release() {
    let f = Fixture::new();
    let path = fixture_journal(&f);
    let j = OperationJournal::open(&path).unwrap();
    let image = j.snapshot().unwrap();
    assert_eq!(image.schema_version, JOURNAL_SCHEMA);
    assert_eq!(image.readable_by().unwrap(), JOURNAL_SCHEMA);
    assert_eq!(
        image.operations.keys().collect::<Vec<_>>(),
        ["logical-op", "pending-op", "queued-op"]
    );
    let finished = &image.operations["logical-op"];
    assert_eq!(finished.attempts.len(), 2);
    assert_eq!(
        finished.attempts[0].outcome,
        AttemptOutcome::ExpiredUnexecuted
    );
    assert!(finished.finalized_effect().unwrap().is_some());
    assert_eq!(
        image.operations["pending-op"].attempts[0].outcome,
        AttemptOutcome::Unresolved
    );
    // Ordinary writes keep schema 3 and add no key, so the earlier release
    // can still open the journal after a rollback.
    j.control(JournalControl {
        submissions_paused: true,
        ..Default::default()
    })
    .unwrap();
    let written = raw(&path);
    assert_eq!(written["schema_version"], 3);
    let mut keys = written
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    keys.sort();
    assert_eq!(
        keys,
        [
            "checkpoints",
            "control",
            "operations",
            "revision",
            "schema_version"
        ]
    );
    let mut before = raw(&path);
    before["revision"] = json!(0);
    let mut original: Value = serde_json::from_slice(FIXTURE).unwrap();
    original["revision"] = json!(0);
    original["control"]["submissions_paused"] = json!(true);
    assert_eq!(before, original);
}

#[tokio::test]
async fn compaction_keeps_identity_and_effect_and_refuses_redispatch() {
    let f = Fixture::new();
    let path = fixture_journal(&f);
    let j = OperationJournal::open(&path).unwrap();
    let before = j.snapshot().unwrap();
    let effect = before.operations["logical-op"]
        .finalized_effect()
        .unwrap()
        .unwrap()
        .clone();
    let expired = before.operations["logical-op"].attempts[0].clone();
    let AttemptOutcome::Finalized { number, .. } = effect.outcome else {
        panic!("fixture effect is finalized");
    };

    // Too recent, out of scope or not selected: nothing is written.
    for (policy, select) in [
        (CompactionPolicy::finalized_before(number), true),
        (
            CompactionPolicy::finalized_before(number + 1).scope("other"),
            true,
        ),
        (CompactionPolicy::finalized_before(number + 1), false),
    ] {
        let report = j.compact(&policy, |_| select).unwrap();
        assert!(report.compacted.is_empty());
        assert_eq!(report.retained, 3);
        assert_eq!(j.snapshot().unwrap().revision, before.revision);
    }

    let report = j
        .compact(&CompactionPolicy::finalized_before(number + 1), |_| true)
        .unwrap();
    assert_eq!(report.compacted, ["logical-op"]);
    assert_eq!(report.dropped_attempts, 1);
    assert_eq!(report.retained, 2);
    drop(j);

    let written = raw(&path);
    assert_eq!(written["schema_version"], 4);
    assert_eq!(written[JOURNAL_READABLE_BY], 4);
    let j = OperationJournal::open(&path).unwrap();
    let image = j.snapshot().unwrap();
    assert_eq!(image.revision, before.revision + 1);
    assert!(!image.operations.contains_key("logical-op"));
    // Unresolved and unselected work is never compacted.
    for id in ["pending-op", "queued-op"] {
        assert_eq!(
            serde_json::to_value(&image.operations[id]).unwrap(),
            serde_json::to_value(&before.operations[id]).unwrap()
        );
    }
    let done = &image.compacted["logical-op"];
    assert_eq!(done.intent, before.operations["logical-op"].intent);
    assert_eq!(done.effect, effect, "effect attempt is retained exactly");
    assert_eq!(done.revision, image.revision);
    assert_eq!(
        done.dropped,
        [DroppedAttempt {
            id: expired.id,
            hash: expired.hash,
            role: AttemptRole::Ordinary,
            outcome: AttemptOutcome::ExpiredUnexecuted,
            extensions: Default::default(),
        }]
    );
    assert!(!format!("{done:?}").contains("sensitive"));

    // The ID stays idempotent: same intent is finished, different is refused.
    let intent = logical_intent(&image);
    assert_eq!(j.begin(intent.clone()).err(), Some(JournalError::Compacted));
    let mut changed = intent;
    changed.effects = json!({"action": "remark", "input": "other"});
    assert_eq!(j.begin(changed).err(), Some(JournalError::Identity));
    assert_eq!(j.bind("logical-op").err(), Some(JournalError::Compacted));
    assert_eq!(j.snapshot().unwrap().revision, image.revision);

    // Compacting again finds nothing; a schema-4 image stays schema 4.
    let again = j
        .compact(&CompactionPolicy::finalized_before(u64::MAX), |_| true)
        .unwrap();
    assert!(again.compacted.is_empty());
    j.control(JournalControl::default()).unwrap();
    assert_eq!(raw(&path)["schema_version"], 4);
}

#[test]
fn compaction_transitions_cannot_erase_or_forge_evidence() {
    let f = Fixture::new();
    let path = fixture_journal(&f);
    let j = OperationJournal::open(&path).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    // Removing a finished operation without its compacted form.
    assert!(
        j.update(|image| {
            image.operations.remove("logical-op");
            Ok(())
        })
        .is_err()
    );
    // Compacting unresolved or unselected work by hand.
    for id in ["pending-op", "queued-op"] {
        assert!(
            j.update(|image| {
                let finished = image.operations["logical-op"].attempts[1].clone();
                let mut record = image.operations.remove(id).unwrap();
                let fake = record.attempts.pop();
                image.schema_version = 4;
                image.compacted.insert(
                    id.into(),
                    CompactedOperation {
                        intent: record.intent.clone(),
                        mode: record.mode.clone(),
                        effect: fake.unwrap_or(finished),
                        dropped: Vec::new(),
                        record_extensions: Default::default(),
                        revision: image.revision + 1,
                        extensions: Default::default(),
                    },
                );
                Ok(())
            })
            .is_err()
        );
    }
    // Compacted entries without schema 4, and a schema decrease.
    let mut forged = None;
    assert!(
        j.update(|image| {
            let record = image.operations.remove("logical-op").unwrap();
            let form = super::super::compaction::compacted_form(&record, image.revision + 1)
                .unwrap()
                .unwrap();
            forged = Some(form.clone());
            image.compacted.insert("logical-op".into(), form);
            Ok(())
        })
        .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    j.compact(&CompactionPolicy::finalized_before(u64::MAX), |_| true)
        .unwrap();
    for mutate in [
        (|image: &mut JournalImage| image.schema_version = 3) as fn(&mut JournalImage),
        |image| {
            image.compacted.clear();
        },
        |image| {
            image
                .compacted
                .get_mut("logical-op")
                .unwrap()
                .dropped
                .clear();
        },
        |image| {
            image
                .compacted
                .get_mut("logical-op")
                .unwrap()
                .effect
                .material = json!({"round": 9});
        },
        |image| {
            image.extensions.remove(JOURNAL_READABLE_BY);
            image
                .extensions
                .insert(JOURNAL_READABLE_BY.into(), json!(3));
        },
    ] {
        let bytes = std::fs::read(&path).unwrap();
        assert!(
            j.update(|image| {
                mutate(image);
                Ok(())
            })
            .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    assert!(forged.is_some());
}

#[test]
fn newer_schema_declaring_compatibility_reads_and_keeps_its_fields() {
    let f = Fixture::new();
    let path = fixture_journal(&f);
    let mut newer = raw(&path);
    newer["schema_version"] = json!(7);
    newer[JOURNAL_READABLE_BY] = json!(4);
    newer["future_envelope"] = json!({"kept": true});
    newer["operations"]["queued-op"]["future_record"] = json!([1, 2]);
    newer["operations"]["logical-op"]["attempts"][0]["future_attempt"] = json!("x");
    write_raw(&path, &newer);
    let j = OperationJournal::open(&path).unwrap();
    let image = j.snapshot().unwrap();
    assert_eq!(image.schema_version, 7);
    assert_eq!(image.readable_by().unwrap(), 4);
    j.control(JournalControl {
        submissions_paused: true,
        ..Default::default()
    })
    .unwrap();
    let report = j
        .compact(&CompactionPolicy::finalized_before(u64::MAX), |_| true)
        .unwrap();
    assert_eq!(report.compacted, ["logical-op"]);
    let written = raw(&path);
    assert_eq!(
        written["schema_version"], 7,
        "a newer schema is never lowered"
    );
    assert_eq!(written[JOURNAL_READABLE_BY], 4);
    assert_eq!(written["future_envelope"], json!({"kept": true}));
    assert_eq!(
        written["operations"]["queued-op"]["future_record"],
        json!([1, 2])
    );
    assert_eq!(written["control"]["submissions_paused"], true);
}

#[test]
fn newer_schema_without_compatibility_refuses_with_a_version_error_unchanged() {
    let f = Fixture::new();
    let path = fixture_journal(&f);
    let base = raw(&path);
    for (schema, floor, expected) in [
        (7, None, Some((7, 7))),
        (7, Some(json!(5)), Some((7, 5))),
        (5, Some(json!(5)), Some((5, 5))),
        (7, Some(json!(8)), None),
        (7, Some(json!("4")), None),
        (7, Some(json!(0)), None),
        (3, Some(json!(4)), None),
        (0, None, None),
    ] {
        let mut candidate = base.clone();
        candidate["schema_version"] = json!(schema);
        if let Some(floor) = floor {
            candidate[JOURNAL_READABLE_BY] = floor;
        }
        let bytes = write_raw(&path, &candidate);
        let error = OperationJournal::open(&path).err().unwrap();
        match expected {
            Some((version, readable_by)) => {
                assert_eq!(
                    error,
                    JournalError::Newer {
                        record: "journal".into(),
                        version,
                        readable_by,
                        supported: JOURNAL_SCHEMA_MAX,
                    }
                );
                assert!(error.to_string().contains("newer release"));
            }
            None => assert!(
                matches!(error, JournalError::Storage(_) | JournalError::Schema(_)),
                "{schema}: {error:?}"
            ),
        }
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn record_format_declares_a_compatible_range() {
    #[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
    struct Work {
        a: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        b: Option<u32>,
    }
    let rule = RecordFormat::new("demo", "format", 1);
    let decode = |v: Value| rule.decode::<Work>(&v);
    assert_eq!(decode(json!({"a": 1})).unwrap(), Work { a: 1, b: None });
    assert_eq!(
        decode(json!({"a": 1, "b": 2, "format": 1})).unwrap(),
        Work { a: 1, b: Some(2) }
    );
    // Own format: an unknown field is refused, never dropped.
    assert!(matches!(
        decode(json!({"a": 1, "c": 3})),
        Err(JournalError::Storage(_))
    ));
    // Newer and declared readable: unknown fields are ignored.
    assert_eq!(
        decode(json!({"a": 1, "c": 3, "format": 3, "readable_by": 1})).unwrap(),
        Work { a: 1, b: None }
    );
    // Newer without a compatible declaration: a version error.
    assert_eq!(
        decode(json!({"a": 1, "format": 2})).err(),
        Some(JournalError::Newer {
            record: "demo".into(),
            version: 2,
            readable_by: 2,
            supported: 1,
        })
    );
    assert!(matches!(
        decode(json!({"a": 1, "format": 3, "readable_by": 2})),
        Err(JournalError::Newer { readable_by: 2, .. })
    ));
    // Declarations that contradict themselves are malformed.
    for bad in [
        json!({"a": 1, "format": 1, "readable_by": 2}),
        json!({"a": 1, "format": 0}),
        json!({"a": 1, "format": "2"}),
    ] {
        assert!(matches!(decode(bad), Err(JournalError::Storage(_))));
    }
    // A newer record whose known fields no longer decode is still refused.
    assert!(decode(json!({"a": "x", "format": 2, "readable_by": 1})).is_err());
}
