// Derived from RaoFoundation/subtensor sdk/python/bittensor/reads/subnets.py @ 5749dbfbe4a78ccd3938d46ffae45e059488feb7 (Apache-2.0); modified by rtao.
//! Block-pinned reporting lifecycle, including legacy runtimes and deferred cleanup.
use crate::ReadError;
use futures::TryStreamExt;
use rtao_client::{ClientError, View, ViewExt};
use rtao_core::value_ext::ValueExt;
use rtao_primitives::NetUid;
use rtao_runtime::storage::subtensor_module as st;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// User-visible subnet lifecycle, distinct from pool-emission policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubnetLifecycleState {
    /// Registered but not started.
    Registered,
    /// Started, including root.
    Started,
    /// Queued for automatic payout and cleanup.
    PendingDissolution,
    /// Automatic payout and cleanup is in progress.
    Dissolving,
}
impl SubnetLifecycleState {
    /// Python's stable public string representation.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Registered => "registered",
            Self::Started => "started",
            Self::PendingDissolution => "pending_dissolution",
            Self::Dissolving => "dissolving",
        }
    }
}
fn decode(value: &Value) -> Result<Option<SubnetLifecycleState>, ReadError> {
    if value.is_null() {
        return Ok(None);
    }
    let variant = value.as_str().or_else(|| {
        value
            .as_object()
            .filter(|v| v.len() == 1)
            .and_then(|v| v.keys().next().map(String::as_str))
    });
    match variant {
        Some("Registered" | "registered") => Ok(Some(SubnetLifecycleState::Registered)),
        Some("Started" | "started") => Ok(Some(SubnetLifecycleState::Started)),
        Some("PendingDissolution" | "pending_dissolution") => {
            Ok(Some(SubnetLifecycleState::PendingDissolution))
        }
        Some("Dissolving" | "dissolving") => Ok(Some(SubnetLifecycleState::Dissolving)),
        _ => Err(ReadError::SurfaceChanged {
            descriptor: "SubnetState",
            detail: format!("invalid lifecycle value: {value}"),
        }),
    }
}
// Match Python's missing-field/value fallback, never transport failures.
// A runtime before `SubnetState` (482 and earlier) has no such storage: the
// client reports it as absent from the runtime, which also uses legacy state.
fn legacy_error(error: &ClientError) -> bool {
    matches!(
        error,
        ClientError::SurfaceChanged { .. }
            | ClientError::Decode { .. }
            | ClientError::Core(rtao_core::CoreError::NotInRuntime(_))
    )
}
async fn legacy<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
) -> Result<Option<SubnetLifecycleState>, ReadError> {
    let (status, queue, added) = view
        .fetch((
            st::CURRENT_DISSOLVE_CLEANUP_STATUS.at(()),
            st::DISSOLVE_CLEANUP_QUEUE.at(()),
            st::NETWORKS_ADDED.at((netuid,)),
        ))
        .await?;
    if status
        .as_ref()
        .and_then(|v| v.as_core().field("netuid"))
        .and_then(|v| v.as_u128())
        == Some(u128::from(netuid.0))
    {
        return Ok(Some(SubnetLifecycleState::Dissolving));
    }
    if queue.contains(&netuid) {
        return Ok(Some(SubnetLifecycleState::PendingDissolution));
    }
    if !added {
        return Ok(None);
    }
    if netuid == NetUid::ROOT {
        return Ok(Some(SubnetLifecycleState::Started));
    }
    let (first, enabled) = view
        .fetch((
            st::FIRST_EMISSION_BLOCK_NUMBER.at((netuid,)),
            st::SUBTOKEN_ENABLED.at((netuid,)),
        ))
        .await?;
    Ok(Some(if first.is_some() || enabled {
        SubnetLifecycleState::Started
    } else {
        SubnetLifecycleState::Registered
    }))
}
/// Lifecycle at one immutable block; absent or invalid new storage uses legacy state.
pub async fn subnet_state<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
) -> Result<Option<SubnetLifecycleState>, ReadError> {
    let snapshot = view.pin().await?;
    match snapshot.storage(&st::SUBNET_STATE, (netuid,)).await {
        Ok(Some(value)) => {
            if let Ok(Some(state)) = decode(&value.to_json()) {
                return Ok(Some(state));
            }
        }
        Ok(None) => {}
        Err(error) if legacy_error(&error) => {}
        Err(error) => return Err(error.into()),
    }
    legacy(&snapshot, netuid).await
}
/// All active or dissolving subnets, with legacy fallback at the same block.
pub async fn subnet_states<V: View + ?Sized>(
    view: &V,
) -> Result<BTreeMap<NetUid, SubnetLifecycleState>, ReadError> {
    let snapshot = view.pin().await?;
    match snapshot
        .storage_iter(&st::SUBNET_STATE, ())
        .try_collect::<Vec<_>>()
        .await
    {
        Ok(rows) if !rows.is_empty() => {
            let decoded = rows
                .into_iter()
                .map(|(keys, value)| {
                    let netuid = super::m6_root::subnet_key(&keys, "SubtensorModule.SubnetState")?;
                    Ok(decode(&value.to_json())?.map(|state| (netuid, state)))
                })
                .collect::<Result<Vec<_>, ReadError>>();
            if let Ok(rows) = decoded {
                return Ok(rows.into_iter().flatten().collect());
            }
        }
        Ok(_) => {}
        Err(error) if legacy_error(&error) => {}
        Err(error) => return Err(error.into()),
    }
    let (added, (queue, status)) = futures::try_join!(
        snapshot
            .storage_iter(&st::NETWORKS_ADDED, ())
            .try_collect::<Vec<_>>(),
        snapshot.fetch((
            st::DISSOLVE_CLEANUP_QUEUE.at(()),
            st::CURRENT_DISSOLVE_CLEANUP_STATUS.at(())
        ))
    )?;
    let mut ids = BTreeSet::new();
    for (keys, added) in added {
        if added {
            ids.insert(super::m6_root::subnet_key(
                &keys,
                "SubtensorModule.NetworksAdded",
            )?);
        }
    }
    ids.extend(queue);
    if let Some(status) = status {
        let id = status
            .as_core()
            .field("netuid")
            .and_then(|v| v.as_u128())
            .and_then(|v| u16::try_from(v).ok())
            .ok_or_else(|| ReadError::SurfaceChanged {
                descriptor: "CurrentDissolveCleanupStatus",
                detail: "missing native netuid".into(),
            })?;
        ids.insert(NetUid(id));
    }
    let mut states = BTreeMap::new();
    for id in ids {
        if let Some(state) = legacy(&snapshot, id).await? {
            states.insert(id, state);
        }
    }
    Ok(states)
}
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)] // Invalid fixture projections must fail the test.
    use super::*;
    #[test]
    fn lifecycle_accepts_python_strings_and_scale_variants() {
        for (wire, state) in [
            ("Registered", SubnetLifecycleState::Registered),
            ("Started", SubnetLifecycleState::Started),
            (
                "PendingDissolution",
                SubnetLifecycleState::PendingDissolution,
            ),
            ("Dissolving", SubnetLifecycleState::Dissolving),
        ] {
            assert_eq!(decode(&serde_json::json!(wire)).unwrap(), Some(state));
            assert_eq!(
                decode(&serde_json::json!({wire:null})).unwrap(),
                Some(state)
            );
            assert_eq!(
                decode(&serde_json::json!(state.as_str())).unwrap(),
                Some(state)
            );
            assert_eq!(
                serde_json::to_value(state).unwrap(),
                serde_json::json!(state.as_str())
            );
        }
        assert_eq!(decode(&Value::Null).unwrap(), None);
        for value in [
            serde_json::json!("unknown"),
            serde_json::json!({"Started":null,"Registered":null}),
            serde_json::json!(3),
        ] {
            assert!(decode(&value).is_err());
        }
        assert!(!legacy_error(&ClientError::Protocol(
            "failed request".into()
        )));
    }
}
