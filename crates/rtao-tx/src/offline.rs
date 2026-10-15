// Derived from RaoFoundation/subtensor sdk/python/bittensor/executor.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Public preparation and policy-gated submission of externally signed calls.

use crate::{ExecuteOptions, Executor, ExtrinsicResult, IntentError, TxError};
use rtao_client::{
    CallBytes, Mortality, NoncePolicy, Period, PublicPrepareOptions, SubmitOptions,
    UnsignedExtrinsic,
};
use rtao_primitives::{AccountId, CryptoType};
use rtao_signer::MultiSignature;
use std::time::Duration;

/// Public inputs for an external signing round trip.
#[derive(Clone, Debug)]
pub struct PrepareOptions {
    /// Era period; `None` selects an immortal transaction.
    pub period: Option<u32>,
    /// Explicit nonce, or a fresh chain nonce without a cached lease.
    pub nonce: Option<u64>,
    /// Expected external signature algorithm.
    pub crypto_type: CryptoType,
    /// Native TAO tip in rao.
    pub tip: u64,
    /// Optional fee asset identifier.
    pub tip_asset_id: Option<u64>,
    /// Optional RFC-0078 digest checked against authenticated metadata.
    pub metadata_hash: Option<[u8; 32]>,
    /// Metadata display decimals used to verify a supplied digest.
    pub metadata_decimals: u8,
    /// Metadata display symbol used to verify a supplied digest.
    pub metadata_symbol: String,
    /// Bound nonce lookup and preparation together.
    pub deadline: Option<Duration>,
}

impl Default for PrepareOptions {
    fn default() -> Self {
        let submit = SubmitOptions::default();
        Self {
            period: Some(rtao_catalog::py::DEFAULT_ERA_PERIOD as u32),
            nonce: None,
            crypto_type: CryptoType::Sr25519,
            tip: 0,
            tip_asset_id: None,
            metadata_hash: None,
            metadata_decimals: submit.metadata_decimals,
            metadata_symbol: submit.metadata_symbol,
            deadline: None,
        }
    }
}

impl Executor {
    /// Prepare a composed call using only its public signing identity.
    ///
    /// Calls from a reviewed [`crate::Plan`] retain their exact wrappers. Auto
    /// nonce selection queries fresh chain state without changing the shared
    /// nonce cache; an explicit nonce also reserves no lease. Preparation does
    /// not authorize submission under an active raw-call policy.
    pub async fn prepare_call(
        &self,
        call: CallBytes,
        address: AccountId,
        opts: &PrepareOptions,
    ) -> Result<UnsignedExtrinsic, TxError> {
        self.client()
            .prepare_public(
                call,
                address,
                &PublicPrepareOptions {
                    submit: SubmitOptions {
                        mortality: opts.period.map_or(Mortality::Immortal, |period| {
                            Mortality::Mortal(Period::new(u64::from(period)))
                        }),
                        nonce: opts.nonce.map_or(NoncePolicy::Auto, NoncePolicy::Explicit),
                        deadline: opts.deadline,
                        metadata_decimals: opts.metadata_decimals,
                        metadata_symbol: opts.metadata_symbol.clone(),
                        ..Default::default()
                    },
                    crypto_type: opts.crypto_type,
                    tip: opts.tip,
                    tip_asset_id: opts.tip_asset_id,
                    metadata_hash: opts.metadata_hash,
                },
            )
            .await
            .map_err(TxError::from)
    }

    /// Verify and submit an external signature under the current raw-call policy.
    ///
    /// The client authenticates both runtime stamps, chain/era identity and the
    /// signature before author submission. No signing keys are resolved. Only
    /// policy, wait, deadline and reconciliation options apply here: the signed
    /// nonce, era and call are immutable. There is no replan or retry of a signed
    /// call. Compose proxy, Sudo, multisig or shield wrappers before preparation;
    /// options requesting new proxy/shield wrappers refuse without broadcasting.
    ///
    /// Client-owned deadlines preserve the attempted transaction hash if the
    /// author acknowledgment or subsequent watch leaves the outcome unknown.
    pub async fn submit_signature(
        &self,
        unsigned: &UnsignedExtrinsic,
        signature: MultiSignature,
        opts: &ExecuteOptions,
    ) -> Result<ExtrinsicResult, TxError> {
        self.enforce_raw_call(opts)?;
        if opts.shielded || opts.proxy_for.is_some() || opts.force_proxy_type.is_some() {
            return Err(IntentError::InvalidArgument(
                "external signatures require wrappers to be composed before preparation".into(),
            )
            .into());
        }
        let result = self
            .client()
            .submit_signature(unsigned, signature, &opts.submit_options())
            .await;
        self.lift_submission(result, serde_json::Map::new(), false)
    }
}
