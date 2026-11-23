// Derived from RaoFoundation/subtensor sdk/python/bittensor/client.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/_substrate.py @ a68c6e8c65411a842fbf69269dc385aa13041391 (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/interface.py @ a68c6e8c65411a842fbf69269dc385aa13041391 (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/client.py @ a68c6e8c65411a842fbf69269dc385aa13041391 (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/snapshot.py @ a68c6e8c65411a842fbf69269dc385aa13041391 (Apache-2.0); modified by rtao.
//! Public strict SCALE decoding through the current runtime.

use crate::{BlockRef, Client, ClientError, Snapshot, View, decode_typed};
use rtao_core::DynValue;
use rtao_runtime::models::CommitmentRegistration;

impl Client {
    /// Decode a complete SCALE value by its live metadata name or core type expression.
    ///
    /// `Call` decodes the runtime call enum, including nested wrappers. Unknown
    /// names, malformed SCALE and trailing input return an error.
    pub async fn decode_scale(
        &self,
        type_name: &str,
        bytes: &[u8],
    ) -> Result<DynValue, ClientError> {
        let runtime = View::runtime(self).await?;
        let spec = runtime
            .core()
            .type_spec(type_name)
            .map_err(rtao_core::CoreError::from)?;
        let decode = move |input: &[u8]| {
            runtime
                .core()
                .decode_spec(&spec, input, true)
                .map(DynValue::new)
                .map_err(|source| ClientError::Decode {
                    what: "SCALE value".into(),
                    source: source.into(),
                })
        };
        decode_bytes(bytes, decode).await
    }

    /// Decode a channel registration using the selected block's metadata.
    ///
    /// With no block specified, resolve the current best block.
    pub async fn decode_commitment_registration(
        &self,
        bytes: &[u8],
        block: Option<BlockRef>,
    ) -> Result<CommitmentRegistration, ClientError> {
        self.at(block.unwrap_or(BlockRef::Best))
            .await?
            .decode_commitment_registration(bytes)
            .await
    }
}

impl Snapshot {
    /// Decode complete channel registration bytes against this snapshot's metadata.
    ///
    /// Uses the value type of `Commitments.ChannelCommitmentOf`. A missing
    /// entry, malformed encoding or trailing bytes returns an error.
    pub async fn decode_commitment_registration(
        &self,
        bytes: &[u8],
    ) -> Result<CommitmentRegistration, ClientError> {
        let runtime = self.block().runtime.clone();
        let type_id = runtime
            .storage_entry("Commitments", "ChannelCommitmentOf")
            .ok_or_else(|| {
                rtao_core::CoreError::NotInRuntime("storage Commitments.ChannelCommitmentOf".into())
            })?
            .value_type;
        decode_bytes(bytes, move |input| decode_typed(&runtime, type_id, input)).await
    }
}

async fn decode_bytes<T>(
    bytes: &[u8],
    decode: impl FnOnce(&[u8]) -> Result<T, ClientError> + Send + 'static,
) -> Result<T, ClientError>
where
    T: Send + 'static,
{
    if bytes.len() > 64 * 1024 {
        let bytes = bytes.to_vec();
        crate::offload(move || decode(&bytes))
            .await
            .map_err(|error| ClientError::Protocol(format!("SCALE decoder task failed: {error}")))?
    } else {
        decode(bytes)
    }
}
