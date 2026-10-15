// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/upgrade_helpers.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Snapshot-bound discovery and live-metadata verification.

use super::{artifacts::*, *};
use rtao::{
    client::{Snapshot, ViewExt},
    core::{DynValue, codec::Value as CoreValue},
};
use rtao_runtime::{
    CallBytes, RuntimeHandle, calls,
    models::{MultisigInfo, MultisigTimepoint},
    storage as st,
};
use serde::Serialize;

pub(super) fn weight(ref_time: u64, proof_size: u64) -> DynValue {
    DynValue::new(CoreValue::record(vec![
        ("ref_time".into(), CoreValue::Uint(u128::from(ref_time))),
        ("proof_size".into(), CoreValue::Uint(u128::from(proof_size))),
    ]))
}

pub(super) fn reconstruct(
    runtime: &RuntimeHandle,
    code: &[u8],
    real: AccountId,
) -> Result<CallBytes, CliError> {
    let code = calls::system::set_code(code.to_vec())
        .compose(runtime)
        .map_err(failure)?;
    let sudo =
        calls::sudo::sudo_unchecked_weight(code, weight(PROPOSAL_REF_TIME, PROPOSAL_PROOF_SIZE))
            .compose(runtime)
            .map_err(failure)?;
    rtao::client::compose::proxy(runtime, real, None, sudo).map_err(failure)
}

pub(super) fn finalizing(
    runtime: &RuntimeHandle,
    blob: &[u8],
    pending: &Value,
) -> Result<CallBytes, CliError> {
    let ci: AccountId = serde_json::from_value(pending["ci_address"].clone()).map_err(failure)?;
    let when: MultisigTimepoint =
        serde_json::from_value(pending["timepoint"].clone()).map_err(failure)?;
    let proposal = CallBytes::decode(blob, runtime).map_err(failure)?;
    calls::multisig::as_multi(
        2,
        vec![ci],
        Some(when),
        proposal,
        weight(FINALIZE_REF_TIME, FINALIZE_PROOF_SIZE),
    )
    .compose(runtime)
    .map_err(failure)
}

pub(super) async fn sudo_layer(
    snapshot: &Snapshot,
    sudo: AccountId,
    call_hash: [u8; 32],
) -> Result<Option<MultisigInfo>, CliError> {
    snapshot
        .storage(&st::multisig::MULTISIGS, (sudo, call_hash))
        .await
        .map_err(failure)
}

pub(super) async fn discover(snapshot: &Snapshot, sudo: AccountId) -> Result<Vec<Value>, CliError> {
    crate::commands::multisig::discover_pending_upgrades(snapshot, sudo).await
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct Check {
    pub name: &'static str,
    pub ok: Option<bool>,
    pub detail: String,
}
#[derive(Clone, Debug)]
pub(super) struct Outcome {
    pub checks: Vec<Check>,
    pub data: Map<String, Value>,
    pub sudo: AccountId,
    pub pending: Option<Value>,
}
impl Outcome {
    pub fn ok(&self) -> bool {
        self.checks.iter().all(|check| check.ok != Some(false))
    }
    pub fn payload(&self) -> Value {
        json!({"ok":self.ok(),"checks":self.checks,"data":self.data})
    }
}

pub(super) async fn verify(
    snapshot: &Snapshot,
    bundle: &ProposalBundle,
) -> Result<Outcome, CliError> {
    let sudo = snapshot
        .storage(&st::sudo::KEY, ())
        .await
        .map_err(failure)?
        .ok_or_else(|| failure("chain has no sudo key"))?;
    let hash = bundle.call_hash()?;
    let pending = discover(snapshot, sudo).await?.into_iter().find(|record| {
        record["call_hash"]
            .as_str()
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(&hash))
    });
    let mut outcome = Outcome {
        checks: Vec::new(),
        data: Map::from_iter([
            ("call_hash".into(), json!(hash)),
            ("sudo_key".into(), json!(sudo)),
        ]),
        sudo,
        pending,
    };
    let mut check = |name, ok, detail: String| outcome.checks.push(Check { name, ok, detail });
    match parse_proposal_blob(&bundle.blob) {
        Err(error) => check("call structure", Some(false), error.to_string()),
        Ok(parsed) => {
            check(
                "call structure",
                Some(true),
                format!(
                    "exactly proxy.proxy(real, None, sudoUncheckedWeight(setCode(code), {{ref_time: {PROPOSAL_REF_TIME}, proof_size: {PROPOSAL_PROOF_SIZE}}})); runtime is {} bytes",
                    parsed.code.len()
                ),
            );
            outcome
                .data
                .insert("proxied_account".into(), json!(parsed.real));
            outcome.data.insert(
                "code_sha256".into(),
                json!(format!("0x{}", sha256_hex(parsed.code))),
            );
            let reconstructed = reconstruct(&snapshot.block().runtime, parsed.code, parsed.real)?;
            let equal = reconstructed.as_bytes() == bundle.blob;
            check(
                "re-encoding",
                Some(equal),
                if equal {
                    "composing the same call against live chain metadata reproduces the call data byte-for-byte".into()
                } else {
                    format!(
                        "re-encoded call differs from the call data ({} vs {} bytes) — runtime metadata may disagree with the offline parse",
                        reconstructed.as_bytes().len(),
                        bundle.blob.len()
                    )
                },
            );
            match &bundle.wasm {
                Some(wasm) => {
                    let equal = wasm.as_slice() == parsed.code;
                    let detail=if equal {
                        let source=if bundle.wasm_source==Some("local") {"your local build"} else {"the release's subtensor.wasm"};
                        format!("embedded runtime is byte-identical to {source}")
                    } else {
                        format!("embedded runtime differs from the provided wasm (sha256 {} vs {})",sha256_hex(wasm),sha256_hex(parsed.code))
                    };
                    check("runtime match",Some(equal),detail);
                }
                None => check("runtime match",None,"no wasm provided — build from source with srtool and pass --wasm to pin the call data to code you compiled yourself".into()),
            }
            if let Some(digest) = &bundle.digest {
                let expected = digest
                    .get("sha256")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim_start_matches("0x");
                let equal = sha256_hex(parsed.code)
                    .trim_start_matches("0x")
                    .eq_ignore_ascii_case(expected);
                check(
                    "srtool digest",
                    Some(equal),
                    format!(
                        "sha256 {} subtensor-digest.json",
                        if equal { "matches" } else { "differs from" }
                    ),
                );
            }
            let equal = parsed.real == sudo;
            check(
                "sudo key",
                Some(equal),
                format!(
                    "proxied account {} {} the chain's sudo key {sudo}",
                    parsed.real,
                    if equal { "is" } else { "IS NOT" }
                ),
            );
        }
    }
    if let Some(pending) = &outcome.pending {
        outcome.data.insert("pending".into(), pending.clone());
        check(
            "on-chain proposal",
            Some(true),
            format!(
                "pending deployment-multisig op matches blake2_256(call data) {hash} (proposed at {} by {})",
                pending["timepoint_display"].as_str().unwrap_or(""),
                pending["ci_address"].as_str().unwrap_or("")
            ),
        );
    } else {
        check(
            "on-chain proposal",
            Some(false),
            format!(
                "no pending deployment-multisig op has call hash {hash} (already executed, cancelled, or not proposed on this network)"
            ),
        );
    }
    if let Some(manifest) = bundle
        .manifest
        .as_ref()
        .filter(|value| value.as_object().is_some_and(|object| !object.is_empty()))
    {
        let equal = manifest
            .get("call_hash")
            .and_then(Value::as_str)
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(&hash));
        check(
            "manifest call hash",
            Some(equal),
            format!(
                "upgrade-manifest.json call_hash {} the call data",
                if equal { "matches" } else { "differs from" }
            ),
        );
        if let (Some(pending), Some(ci)) = (
            &outcome.pending,
            manifest
                .get("ci_address")
                .filter(|value| !value.is_null() && value.as_str() != Some("")),
        ) {
            let equal = ci == &pending["ci_address"];
            check(
                "manifest CI key",
                Some(equal),
                format!(
                    "manifest ci_address {} the on-chain depositor",
                    if equal { "matches" } else { "differs from" }
                ),
            );
        }
        outcome.data.insert("manifest".into(),json!({"spec_version":manifest.get("spec_version"),"commit":manifest.get("commit"),"tag":manifest.get("tag"),"repo":manifest.get("repo"),"wasm_sha256":manifest.get("wasm_sha256")}));
    }
    Ok(outcome)
}
