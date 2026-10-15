// Derived from RaoFoundation/subtensor sdk/python/bittensor/client.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Public strict SCALE decoding through the current runtime.

use crate::{Client, ClientError, View};
use rtao_core::DynValue;

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
        if bytes.len() > 64 * 1024 {
            let bytes = bytes.to_vec();
            crate::offload(move || decode(&bytes))
                .await
                .map_err(|error| {
                    ClientError::Protocol(format!("SCALE decoder task failed: {error}"))
                })?
        } else {
            decode(bytes)
        }
    }
}
