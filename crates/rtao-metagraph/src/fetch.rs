// Derived from RaoFoundation/subtensor sdk/python/bittensor/metagraph.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/namespaces.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Curated graph reads sharing a single block with the commitment scans.

use std::future::Future;

use rtao_client::{ClientError, View};
use rtao_core::{CoreError, codec::Value, ops::runtime_api};
use rtao_primitives::{MechId, NetUid};
use rtao_reads::{
    ReadError, Subnets,
    commitments::{CommitmentMap, fetch_commitment_map},
};
use rtao_runtime::SelectiveMetagraphIndex;

use crate::{Metagraph, MetagraphError, MetagraphOptions, SnapshotInfo};

/// The curated metagraph methods on the shared subnet namespace.
pub trait SubnetsMetagraphExt {
    /// Fetch a full mechanism graph and optional commitments at one block.
    fn mechagraph_with(
        &self,
        netuid: NetUid,
        mechid: MechId,
        options: MetagraphOptions,
    ) -> impl Future<Output = Result<Option<Metagraph>, ReadError>> + Send;
    /// Fetch selected fields for one mechanism, retaining the partial raw record.
    fn selective_metagraph_with(
        &self,
        netuid: NetUid,
        mechid: MechId,
        fields: &[SelectiveMetagraphIndex],
        options: MetagraphOptions,
    ) -> impl Future<Output = Result<Option<Metagraph>, ReadError>> + Send;
    /// Fetch a subnet graph and commitments pinned to one immutable block.
    fn metagraph(
        &self,
        netuid: NetUid,
    ) -> impl Future<Output = Result<Option<Metagraph>, ReadError>> + Send;
    /// Fetch a graph with explicit commitment and decoder options.
    fn metagraph_with(
        &self,
        netuid: NetUid,
        options: MetagraphOptions,
    ) -> impl Future<Output = Result<Option<Metagraph>, ReadError>> + Send;
}

impl<V: View> SubnetsMetagraphExt for Subnets<'_, V> {
    async fn mechagraph_with(
        &self,
        netuid: NetUid,
        mechid: MechId,
        options: MetagraphOptions,
    ) -> Result<Option<Metagraph>, ReadError> {
        fetch(self.view(), netuid, Query::Mechanism(mechid), options).await
    }
    async fn selective_metagraph_with(
        &self,
        netuid: NetUid,
        mechid: MechId,
        fields: &[SelectiveMetagraphIndex],
        options: MetagraphOptions,
    ) -> Result<Option<Metagraph>, ReadError> {
        fetch(
            self.view(),
            netuid,
            Query::Selective(mechid, fields.to_vec()),
            options,
        )
        .await
    }
    async fn metagraph(&self, netuid: NetUid) -> Result<Option<Metagraph>, ReadError> {
        self.metagraph_with(netuid, MetagraphOptions::default())
            .await
    }
    async fn metagraph_with(
        &self,
        netuid: NetUid,
        options: MetagraphOptions,
    ) -> Result<Option<Metagraph>, ReadError> {
        fetch(self.view(), netuid, Query::Standard, options).await
    }
}

enum Query {
    Standard,
    Mechanism(MechId),
    Selective(MechId, Vec<SelectiveMetagraphIndex>),
}

async fn fetch(
    view: &impl View,
    netuid: NetUid,
    query: Query,
    mut options: MetagraphOptions,
) -> Result<Option<Metagraph>, ReadError> {
    NetUid::new(netuid.0).map_err(|error| ReadError::InvalidParam {
        name: "netuid".into(),
        reason: error.to_string(),
    })?;
    let snapshot = view.pin().await?;
    options.view = options.view
        && view.metagraph_view_enabled()
        && snapshot.metagraph_view_enabled()
        && std::env::var("RTAO_METAGRAPH_VIEW").as_deref() != Ok("0");
    if !options.view && options.decoder == crate::DecodePreference::View {
        return Err(ReadError::InvalidParam {
            name: "decoder".into(),
            reason: "metagraph view is disabled".into(),
        });
    }
    let block = snapshot.block();
    let runtime = block.runtime.clone();
    let at = SnapshotInfo {
        number: block.number,
        hash: block.hash,
        spec_version: runtime.core().spec_version,
    };
    let (name, params) = match &query {
        Query::Standard => ("get_metagraph", vec![Value::Int(i128::from(netuid.0))]),
        Query::Mechanism(mechid) => (
            "get_mechagraph",
            vec![
                Value::Int(i128::from(netuid.0)),
                Value::Int(i128::from(mechid.0)),
            ],
        ),
        Query::Selective(mechid, fields) => (
            "get_selective_mechagraph",
            vec![
                Value::Int(i128::from(netuid.0)),
                Value::Int(i128::from(mechid.0)),
                Value::List(
                    fields
                        .iter()
                        .map(|field| Value::Int(i128::from(field.index())))
                        .collect(),
                ),
            ],
        ),
    };
    if let Query::Mechanism(mechid) | Query::Selective(mechid, _) = &query {
        MechId::new(mechid.0).map_err(|error| ReadError::InvalidParam {
            name: "mechid".into(),
            reason: error.to_string(),
        })?;
        let have = runtime
            .runtime_api_version("SubnetInfoRuntimeApi")
            .unwrap_or(0);
        if have < 2 {
            return Err(ClientError::ApiVersion {
                api: "SubnetInfoRuntimeApi".into(),
                need: 2,
                have,
            }
            .into());
        }
    }
    let method = runtime_api::runtime_api_method(runtime.core(), "SubnetInfoRuntimeApi", name)
        .map_err(CoreError::from)
        .map_err(ClientError::from)?;
    let args = runtime_api::encode_params(runtime.core(), "SubnetInfoRuntimeApi", method, &params)
        .map_err(CoreError::from)
        .map_err(ClientError::from)?;
    let graph = async {
        snapshot
            .state_call(&format!("SubnetInfoRuntimeApi_{name}"), &args)
            .await
            .map_err(ReadError::from)
    };
    let commitments = async {
        if options.commitments {
            fetch_commitment_map(&snapshot, netuid, false).await
        } else {
            Ok(CommitmentMap::default())
        }
    };
    let (raw, commitments) = tokio::try_join!(graph, commitments)?;
    #[cfg(feature = "persist")]
    let persistence_source = match &query {
        Query::Standard => crate::persist::Source::FullSubnet,
        Query::Mechanism(_) => crate::persist::Source::FullMechanism,
        Query::Selective(_, fields) => {
            crate::persist::Source::Selective(fields.iter().map(|field| field.index()).collect())
        }
    };
    // Large owned/dynamic decoding runs away from the reactor. Borrowed parsing
    // is small, but this common threshold also bounds JSON fallback work.
    let offload = raw.len() > 64 * 1024;
    let decode = move || match query {
        Query::Standard => Metagraph::decode(raw, &runtime, netuid, at, options),
        Query::Mechanism(_) => Metagraph::decode_mechanism(raw, &runtime, netuid, at, options),
        Query::Selective(mechid, _) => {
            Metagraph::decode_selective(raw, &runtime, netuid, mechid, at, options)
        }
    };
    let mut graph = if offload {
        tokio::task::spawn_blocking(decode).await.map_err(|error| {
            ClientError::Protocol(format!("metagraph decoder task failed: {error}"))
        })?
    } else {
        decode()
    }
    .map_err(|error| read_error(error, name))?;
    #[cfg(feature = "persist")]
    if let Some(graph) = &mut graph {
        graph.set_persistence_origin(snapshot.client().genesis(), persistence_source);
    }
    if options.commitments
        && let Some(graph) = &mut graph
    {
        graph.assign_commitments(commitments);
    }
    Ok(graph)
}

fn read_error(error: MetagraphError, name: &str) -> ReadError {
    match error {
        MetagraphError::Shape(detail) => ReadError::SurfaceChanged {
            descriptor: match name {
                "get_mechagraph" => "SubnetInfoRuntimeApi.get_mechagraph",
                "get_selective_mechagraph" => "SubnetInfoRuntimeApi.get_selective_mechagraph",
                _ => "SubnetInfoRuntimeApi.get_metagraph",
            },
            detail,
        },
        MetagraphError::Core(error) => ReadError::Client(ClientError::from(error)),
        error => ReadError::Client(ClientError::Protocol(error.to_string())),
    }
}
