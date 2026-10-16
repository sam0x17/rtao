// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/multisig_helpers.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Advisory followup for direct generated multisig approval/execute intents.

use super::AppContext;
use crate::{
    books::{AccountLookup, FsAccountLookup},
    call::{FsRawCallCache, MultisigFollowup, RawMultisig},
    error::CliError,
};
use rtao::{
    client::{BlockRef, Client, View, config::MAX_JSON_FILE_BYTES},
    primitives::AccountId,
    tx::{DynIntent, ExtrinsicResult, IntentOutput, Multisig, MultisigCall},
};
use rtao_runtime::CallBytes;
use serde_json::Value;
use std::collections::BTreeSet;

pub(super) fn eligible(intent: &dyn DynIntent, result: &ExtrinsicResult) -> bool {
    result.success
        && ["multisig_approve", "multisig_execute"].contains(&intent.spec().name)
        && result.data.contains_key("multisig_call_hash")
}

fn failure(error: impl std::fmt::Display) -> CliError {
    CliError::failure(error.to_string())
}

// Python's soft book matching ignores unresolved references and preserves the
// first matching entry and its portable refs. It never opens a private key.
fn saved_group(
    ctx: &AppContext,
    signatories: &[AccountId],
    threshold: u16,
) -> Result<(Option<String>, Vec<String>), CliError> {
    let lookup = FsAccountLookup::new(ctx.globals.wallet_path.clone()).map_err(failure)?;
    let wanted: BTreeSet<_> = signatories.iter().copied().collect();
    for entry in &ctx.books.multisigs {
        if entry.get("threshold").and_then(Value::as_u64) != Some(u64::from(threshold)) {
            continue;
        }
        let Some(name) = entry.get("name").and_then(Value::as_str) else {
            continue;
        };
        let Some(refs) = entry.get("signatories").and_then(Value::as_array) else {
            continue;
        };
        let Some(refs) = refs.iter().map(Value::as_str).collect::<Option<Vec<_>>>() else {
            continue;
        };
        let resolved: BTreeSet<AccountId> = refs
            .iter()
            .filter_map(|reference| {
                reference.parse().ok().or_else(|| {
                    ctx.books
                        .addresses
                        .iter()
                        .find(|entry| entry.get("name").and_then(Value::as_str) == Some(reference))
                        .and_then(|entry| entry.get("address"))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .or_else(|| lookup.coldkey(reference).ok())
                        .and_then(|address| address.parse().ok())
                })
            })
            .collect();
        if resolved == wanted {
            return Ok((
                Some(name.to_owned()),
                refs.into_iter().map(str::to_owned).collect(),
            ));
        }
    }
    Ok((None, signatories.iter().map(ToString::to_string).collect()))
}

pub(super) async fn followup(
    ctx: &AppContext,
    client: &Client,
    intent: &dyn DynIntent,
    signer: AccountId,
    result: &ExtrinsicResult,
) -> Result<Option<Value>, CliError> {
    if !eligible(intent, result) {
        return Ok(None);
    }
    let args = intent.to_json_dyn();
    let threshold = args
        .get("threshold")
        .and_then(Value::as_u64)
        .and_then(|value| u16::try_from(value).ok())
        .filter(|threshold| *threshold >= 2)
        .ok_or_else(|| failure("direct multisig followup requires a native threshold"))?;
    let role = intent.signer_role_dyn().map_err(failure)?;
    let mut signatories = args
        .get("other_signatories")
        .and_then(Value::as_array)
        .ok_or_else(|| failure("direct multisig followup lacks other signatories"))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| failure("direct multisig member is not an address"))?
                .parse::<AccountId>()
                .map_err(failure)
        })
        .collect::<Result<Vec<_>, _>>()?;
    signatories.push(signer);
    // Preserve the intent's member order while removing repeated addresses.
    let mut seen = BTreeSet::new();
    signatories.retain(|member| seen.insert(*member));
    let data = result
        .data
        .get("multisig_call_data")
        .and_then(Value::as_str);
    if data.is_none_or(|value| value.len() > MAX_JSON_FILE_BYTES) {
        return Err(failure(
            "direct multisig call data is absent or exceeds the local budget",
        ));
    }
    let output = MultisigCall::from_data(&result.data).map_err(failure)?;
    let bytes = output
        .call_data
        .ok_or_else(|| failure("missing multisig call bytes"))?;
    let hash = output
        .call_hash
        .ok_or_else(|| failure("missing multisig call hash"))?;
    // Decode at inclusion when known. Hash equality alone cannot authenticate
    // SCALE, and a moving latest runtime must not reinterpret included bytes.
    let snapshot = match result.block_hash {
        Some(hash) => client.at(BlockRef::Hash(hash)).await,
        None => client.pin().await,
    }
    .map_err(CliError::from)?;
    let runtime = &snapshot.block().runtime;
    let call = CallBytes::decode(&bytes, runtime).map_err(failure)?;
    if call.hash() != hash.0 {
        return Err(failure(
            "direct multisig receipt hash does not match its call bytes",
        ));
    }
    let multisig = Multisig::new(runtime, &signatories, threshold).map_err(failure)?;
    let (preset, refs) = saved_group(ctx, &signatories, threshold)?;
    let group = RawMultisig {
        threshold,
        signatories,
        preset,
        refs,
    };
    let spec = crate::call::decode_multisig_call_spec_at(runtime, &call, role);
    let cache = match &ctx.config_store {
        Some(store) => FsRawCallCache::new(store.paths()),
        None => FsRawCallCache::from_process()?,
    };
    crate::call::multisig_followup_for_spec(
        ctx,
        client,
        MultisigFollowup {
            spec: &spec,
            group: &group,
            multisig: &multisig,
            call: &call,
            result,
        },
        &cache,
    )
    .await
    .map(Some)
}
