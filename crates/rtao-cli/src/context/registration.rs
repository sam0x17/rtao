// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/context.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/registration.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Live registration costs and bounded progress at the application boundary.

use futures::TryStreamExt;
use rtao::{
    client::{View, ViewExt},
    primitives::{DynBalance, NetUid},
    tx::DynIntent,
};
use rtao_runtime::{runtime_apis as api, storage::subtensor_module as st};

use crate::{
    error::CliError,
    output::{Output, StepState},
};

// py: sdk/python/bittensor/cli/context.py:1405-1433
fn progress(output: &Output, progress: rtao::tx::Progress) -> Result<(), CliError> {
    let fields = &progress.data;
    let deregistered = fields
        .get("deregistered_netuid")
        .and_then(serde_json::Value::as_u64);
    let cleanup = deregistered.filter(|netuid| *netuid != 0).or_else(|| {
        fields
            .get("cleanup_netuid")
            .and_then(serde_json::Value::as_u64)
    });
    let (text, state) = match progress.stage.as_str() {
        "queued" if deregistered.is_some() => (
            format!(
                "capacity is full · deregistering subnet {} before registration",
                cleanup.map_or_else(|| "None".into(), |netuid| netuid.to_string()),
            ),
            StepState::Running,
        ),
        "queued" => (
            cleanup.map_or_else(
                || "registration queued · waiting for deregistration cleanup".into(),
                |netuid| format!("registration queued · waiting for subnet {netuid} cleanup"),
            ),
            StepState::Running,
        ),
        "waiting" => {
            let elapsed = fields
                .get("blocks_since_call")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let subject = cleanup.map_or_else(
                || "cleanup".into(),
                |netuid| format!("subnet {netuid} cleanup"),
            );
            let unit = if elapsed == 1 { "block" } else { "blocks" };
            (
                format!("{subject} · {elapsed} {unit} since call · waiting for NetworkAdded"),
                StepState::Fact,
            )
        }
        "registered" => {
            let number = |key| {
                fields
                    .get(key)
                    .and_then(serde_json::Value::as_u64)
                    .map_or_else(|| "None".into(), |value| value.to_string())
            };
            (
                format!(
                    "NetworkAdded · subnet {} · block {}",
                    number("netuid"),
                    number("block")
                ),
                StepState::Done,
            )
        }
        _ => return Ok(()),
    };
    output
        .step("registration", &text, state)
        .map_err(|error| CliError::failure(error.to_string()))
}

/// Drain bounded progress while preserving the submission future and its receipt.
pub(super) async fn with_progress<F: std::future::Future + ?Sized>(
    output: &Output,
    mut receiver: tokio::sync::mpsc::Receiver<rtao::tx::Progress>,
    mut operation: std::pin::Pin<Box<F>>,
) -> (F::Output, Option<CliError>) {
    let mut render_error = None;
    let mut render = |value| {
        if render_error.is_none() {
            render_error = progress(output, value).err();
        }
    };
    let result = loop {
        tokio::select! {
            result = &mut operation => break result,
            Some(value) = receiver.recv() => render(value),
        }
    };
    // A final stage may already be queued when the operation becomes ready.
    while let Ok(value) = receiver.try_recv() {
        render(value);
    }
    (result, render_error)
}

pub(super) struct RegistrationReview {
    pub suffix: String,
    pub cost: String,
    pub notes: Vec<String>,
}

pub(super) async fn live<V: View + ?Sized>(
    view: &V,
    intent: &dyn DynIntent,
    assume_yes: bool,
) -> Result<Option<RegistrationReview>, CliError> {
    let semantic = intent.semantic_intent_dyn();
    if semantic.spec().name == "register_subnet" {
        return subnet_creation(view, assume_yes).await.map(Some);
    }
    let recycled = match semantic.spec().name {
        "root_register" => true,
        "burned_register" => false,
        _ => return Ok(None),
    };
    let netuid = if recycled {
        NetUid::ROOT
    } else {
        let netuid = semantic
            .to_json_dyn()
            .get("netuid")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u16::try_from(value).ok())
            .ok_or_else(|| CliError::input("registration review requires a native netuid"))?;
        NetUid(netuid)
    };
    let (burn, lock) = rtao_intents::registration::neuron_registration_split(view, netuid)
        .await
        .map_err(|error| CliError::transaction(error.into()))?;
    let burn = DynBalance::from(burn);
    let lock = DynBalance::from(lock);
    let lock_part = if lock.rao == 0 {
        "none".into()
    } else {
        lock.to_string()
    };
    let mut cost = format!("{burn} {}", if recycled { "recycled" } else { "burned" });
    if lock.rao != 0 {
        cost.push_str(&format!(" + {lock} locked as collateral"));
    }
    let lock_line = if lock.rao != 0 {
        format!("lock {lock} as miner collateral")
    } else if recycled {
        "lock none".into()
    } else {
        "lock none (full cost is burned)".into()
    };
    Ok(Some(RegistrationReview {
        suffix: format!(" — burn {burn} · lock {lock_part}"),
        cost,
        notes: vec![
            format!(
                "burn {burn} ({})",
                if recycled {
                    "recycled into issuance"
                } else {
                    "destroyed"
                }
            ),
            lock_line,
        ],
    }))
}

async fn subnet_creation<V: View + ?Sized>(
    view: &V,
    assume_yes: bool,
) -> Result<RegistrationReview, CliError> {
    let snapshot = view.pin().await.map_err(CliError::from)?;
    let quote = snapshot
        .call(
            &api::subnet_registration_runtime_api::GET_NETWORK_REGISTRATION_COST,
            (),
        )
        .await
        .map_err(CliError::from)?;
    let quote = DynBalance::from_rao(quote.0, NetUid::ROOT);
    // Python formats Balance.decimal, not Balance.tao: keep all native rao
    // digits before adding the thousands separators.
    let decimal = quote.to_decimal_string();
    let integer_len = decimal.find('.').unwrap_or(decimal.len());
    let mut grouped = String::with_capacity(decimal.len() + integer_len / 3);
    for (index, digit) in decimal.chars().enumerate() {
        if index != 0 && index < integer_len && (integer_len - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    let notes = if assume_yes {
        Vec::new()
    } else {
        let (networks, (limit, cleanup, registrations)) = futures::try_join!(
            snapshot
                .storage_iter(&st::NETWORKS_ADDED, ())
                .try_collect::<Vec<_>>(),
            snapshot.fetch((
                st::SUBNET_LIMIT.at(()),
                st::DISSOLVE_CLEANUP_QUEUE.at(()),
                st::NETWORK_REGISTRATION_QUEUE.at(()),
            )),
        )
        .map_err(CliError::from)?;
        let mut active = 0usize;
        for (keys, added) in networks {
            let netuid = keys
                .last()
                .and_then(rtao::core::DynValue::as_u128)
                .and_then(|netuid| u16::try_from(netuid).ok())
                .ok_or_else(|| CliError::failure("registration preview has a non-native netuid"))?;
            if netuid != 0 && added {
                active += 1;
            }
        }
        let flow = if active.saturating_add(cleanup.len()) < usize::from(limit) {
            "immediate · no deregistration needed".into()
        } else if cleanup.len() > registrations.len() {
            let mut flow = "queued · waits for deregistration cleanup".to_owned();
            if let Some(target) = cleanup.first() {
                flow.push_str(&format!(" of subnet {target}"));
            }
            flow
        } else {
            match snapshot
                .call(&api::subnet_info_runtime_api::GET_SUBNET_TO_PRUNE, ())
                .await
                .map_err(CliError::from)?
            {
                None => "blocked · no subnet is eligible for deregistration".into(),
                Some(target) => {
                    format!("queued · deregisters subnet {target} before registration")
                }
            }
        };
        vec![flow]
    };
    Ok(RegistrationReview {
        suffix: format!(" for {grouped} TAO"),
        cost: quote.to_string(),
        notes,
    })
}
