//! `POST /sign` and `GET /accounts` handlers, in the normative check order.

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::State,
    response::{IntoResponse, Response},
};
use http::{HeaderMap, StatusCode, header};
use rtao_btauth::server::{Json, VerifiedCaller};
use rtao_core::digest::ChainInfo;
use rtao_primitives::{CryptoType, Ss58Address};
use rtao_runtime::{CallBytes, RuntimeHandle};
use rtao_signer::{SignPurpose, SigningContext, SigningRequest};

use super::{Inner, rebuild::EraInfo};
use crate::wire::{
    AccountEntry, AccountsResponse, ErrorBody, ErrorCode, Hex32, PROTOCOL, SignRequest,
    SignResponse, Ss58, WIRE_SS58_PREFIX, WirePurpose, blake2_256,
};

struct Refusal {
    code: ErrorCode,
    detail: String,
    violations: Option<Vec<String>>,
}

fn refuse(code: ErrorCode, detail: impl Into<String>) -> Refusal {
    Refusal {
        code,
        detail: detail.into(),
        violations: None,
    }
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.code.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (
            status,
            axum::Json(ErrorBody {
                detail: self.detail,
                code: Some(self.code.as_str().into()),
                violations: self.violations,
            }),
        )
            .into_response()
    }
}

fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|mime| {
            let mime = mime.trim().to_ascii_lowercase();
            mime == "application/json"
                || (mime.starts_with("application/") && mime.ends_with("+json"))
        })
}

pub(super) async fn accounts(
    State(inner): State<Arc<Inner>>,
    VerifiedCaller(caller): VerifiedCaller,
) -> Response {
    let grants = inner.allowlist.grants(&caller.hotkey);
    let accounts = inner
        .accounts
        .iter()
        .filter(|(account, _)| grants.is_some_and(|grants| grants.contains(account)))
        .map(|(account, signer)| AccountEntry {
            account: Ss58(*account),
            crypto: signer.crypto_type().into(),
        })
        .collect();
    let mut purposes = vec![WirePurpose::Extrinsic];
    if inner.policy.allow_shielded {
        purposes.extend([WirePurpose::ShieldInner, WirePurpose::ShieldCarrier]);
    }
    axum::Json(AccountsResponse {
        version: PROTOCOL.into(),
        server_key: Ss58(inner.server_key),
        accounts,
        genesis_hash: Hex32(inner.genesis_hash),
        purposes,
    })
    .into_response()
}

pub(super) async fn sign(
    State(inner): State<Arc<Inner>>,
    VerifiedCaller(caller): VerifiedCaller,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let client = Ss58Address::from(caller.hotkey);
    let mut audit = Audit::default();
    let outcome = sign_checked(&inner, &caller.hotkey, &headers, &body, &mut audit).await;
    let result = match &outcome {
        Ok(_) => "signed",
        Err(refusal) => refusal.code.as_str(),
    };
    tracing::info!(
        target: "rtao_remote_signer::audit",
        client = %client,
        account = %Field(&audit.account),
        purpose = audit.purpose.unwrap_or("-"),
        call = %Field(&audit.call),
        nonce = %Field(&audit.nonce),
        era = %Field(&audit.era),
        payload_blake2_256 = %Field(&audit.payload_hash),
        result,
        "remote signing request"
    );
    match outcome {
        Ok(response) => axum::Json(response).into_response(),
        Err(refusal) => refusal.into_response(),
    }
}

/// Audit fields, filled in as each check learns them, so refusals log
/// whatever was already parsed. Never secrets or the whole payload.
#[derive(Default)]
struct Audit {
    account: Option<Ss58Address>,
    purpose: Option<&'static str>,
    call: Option<String>,
    nonce: Option<u64>,
    era: Option<String>,
    payload_hash: Option<String>,
}

/// Display an optional audit field as `-` when it is unknown.
struct Field<'a, T>(&'a Option<T>);

impl<T: std::fmt::Display> std::fmt::Display for Field<'_, T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Some(value) => value.fmt(formatter),
            None => formatter.write_str("-"),
        }
    }
}

fn source_unavailable(what: &'static str) -> impl Fn(super::SourceError) -> Refusal {
    move |error| {
        tracing::warn!(target: "rtao_remote_signer::audit", error = %error, "{what} failed");
        refuse(
            ErrorCode::ChainUnavailable,
            format!("the trusted chain source failed: {what}"),
        )
    }
}

async fn trusted_runtime(inner: &Inner, request: &SignRequest) -> Result<RuntimeHandle, Refusal> {
    let matches = |runtime: &RuntimeHandle| {
        (
            request.chain.spec_version,
            request.chain.transaction_version,
        ) == (runtime.spec_version(), runtime.transaction_version())
    };
    let mut runtime = inner
        .source
        .current()
        .await
        .map_err(source_unavailable("trusted runtime lookup"))?;
    if !matches(&runtime) && inner.source.is_online() {
        runtime = inner
            .source
            .refresh()
            .await
            .map_err(source_unavailable("trusted runtime refresh"))?;
    }
    if !matches(&runtime) {
        return Err(refuse(
            ErrorCode::RuntimeMismatch,
            format!(
                "request was prepared for spec {} / tx {}, but the server signs for spec {} / tx {}",
                request.chain.spec_version,
                request.chain.transaction_version,
                runtime.spec_version(),
                runtime.transaction_version()
            ),
        ));
    }
    Ok(runtime)
}

/// Online era anchoring, cheapest check first.
///
/// The anchor's number comes from its header alone (no runtime resolution
/// for the client-chosen hash). The phase and the live window are checked
/// before the canonical-hash lookup.
async fn check_era_anchor(
    inner: &Inner,
    era: EraInfo,
    period: u64,
    anchor: [u8; 32],
) -> Result<(), Refusal> {
    let source = &inner.source;
    let Some(number) = source
        .block_number(anchor)
        .await
        .map_err(source_unavailable("era block lookup"))?
    else {
        return Err(refuse(
            ErrorCode::EraRejected,
            "era block is unknown to the server's node",
        ));
    };
    let best = source
        .best_number()
        .await
        .map_err(source_unavailable("best block lookup"))?;
    let live = number <= best
        && number
            .checked_add(period)
            .is_some_and(|expiry| best < expiry);
    if !era.born_at(number) || !live {
        return Err(refuse(
            ErrorCode::EraRejected,
            "era block does not match the era phase or the era has expired",
        ));
    }
    let canonical = source
        .canonical_hash(number)
        .await
        .map_err(source_unavailable("canonical block lookup"))?;
    if canonical != Some(anchor) {
        return Err(refuse(
            ErrorCode::EraRejected,
            "era block is not on the canonical chain",
        ));
    }
    Ok(())
}

async fn sign_checked(
    inner: &Inner,
    client: &rtao_primitives::AccountId,
    headers: &HeaderMap,
    body: &[u8],
    audit: &mut Audit,
) -> Result<SignResponse, Refusal> {
    // 1. Parse. Diagnostics never echo request content.
    if !is_json(headers) {
        return Err(refuse(
            ErrorCode::MalformedRequest,
            "expected Content-Type: application/json",
        ));
    }
    let Json(request) = Json::<SignRequest>::from_bytes(body).map_err(|rejection| {
        tracing::debug!(target: "rtao_remote_signer::audit", error = %rejection, "malformed request body");
        refuse(
            ErrorCode::MalformedRequest,
            match rejection {
                rtao_btauth::server::JsonRejection::Data(_) => {
                    format!("request body does not match the {PROTOCOL} schema")
                }
                _ => "request body is not valid JSON".to_owned(),
            },
        )
    })?;
    if request.version != PROTOCOL {
        return Err(refuse(
            ErrorCode::MalformedRequest,
            format!("unsupported protocol version; expected {PROTOCOL}"),
        ));
    }
    let account = request.account.0;
    audit.account = Some(Ss58Address::from(account));
    audit.purpose = Some(request.purpose.as_str());
    // 2. Account.
    let signer = inner
        .accounts
        .get(&account)
        .filter(|_| {
            inner
                .allowlist
                .grants(client)
                .is_some_and(|grants| grants.contains(&account))
        })
        .ok_or_else(|| {
            refuse(
                ErrorCode::AccountNotAuthorized,
                "this client key is not authorized for the requested account",
            )
        })?;
    let crypto = signer.crypto_type();
    if CryptoType::from(request.crypto) != crypto {
        return Err(refuse(
            ErrorCode::AccountNotAuthorized,
            "the requested signature algorithm is not the account's algorithm",
        ));
    }
    // 3. Purpose.
    if request.purpose != WirePurpose::Extrinsic && !inner.policy.allow_shielded {
        return Err(refuse(
            ErrorCode::PurposeNotAllowed,
            format!(
                "purpose {} is disabled on this server",
                request.purpose.as_str()
            ),
        ));
    }
    // 4. Chain.
    if request.chain.genesis_hash.0 != inner.genesis_hash {
        return Err(refuse(
            ErrorCode::RuntimeMismatch,
            "request was prepared for a different chain",
        ));
    }
    let runtime = trusted_runtime(inner, &request).await?;
    // 5. Call.
    let call = CallBytes::decode(&request.call.0, &runtime).map_err(|error| {
        tracing::debug!(target: "rtao_remote_signer::audit", error = %error, "undecodable call");
        refuse(
            ErrorCode::UndecodableCall,
            "call does not decode canonically under the trusted runtime",
        )
    })?;
    // 6. Metadata identity (diagnostic).
    if request.chain.metadata_blake2_256.0 != blake2_256(&runtime.core().metadata_bytes) {
        return Err(refuse(
            ErrorCode::RuntimeMismatch,
            "client metadata differs from the server's trusted runtime",
        ));
    }
    // 7-9. Parse the JSON view and re-derive extension bytes and payload.
    let rebuilt = super::rebuild::rebuild(&runtime, &request).map_err(|error| match error {
        super::RebuildError::Malformed(message) => refuse(ErrorCode::MalformedRequest, message),
        super::RebuildError::Tampered(message) => refuse(ErrorCode::TamperedRequest, message),
    })?;
    audit.nonce = Some(rebuilt.params.nonce);
    audit.era = Some(rebuilt.era.to_string());
    audit.payload_hash = Some(hex::encode(blake2_256(&rebuilt.payload)));
    // 10. Metadata digest.
    let spec_name = inner
        .source
        .spec_name()
        .await
        .map_err(source_unavailable("runtime name lookup"))?;
    let chain_info = ChainInfo {
        spec_version: runtime.spec_version(),
        spec_name,
        base58_prefix: runtime.core().ss58_format,
        decimals: inner.chain_info.decimals,
        token_symbol: inner.chain_info.token_symbol.clone(),
    };
    match rebuilt.params.metadata_hash {
        Some(digest) => {
            // A failure here is the server's, not the client's.
            let expected = runtime.metadata_digest(&chain_info).map_err(|error| {
                tracing::warn!(target: "rtao_remote_signer::audit", error = %error, "metadata digest failed");
                refuse(
                    ErrorCode::SigningFailed,
                    "the server cannot compute its trusted runtime's metadata digest",
                )
            })?;
            if digest != expected {
                return Err(refuse(
                    ErrorCode::TamperedRequest,
                    "metadataHash is not the trusted runtime's RFC-0078 digest",
                ));
            }
        }
        None if inner.policy.require_metadata_hash => {
            return Err(refuse(
                ErrorCode::TamperedRequest,
                "this server requires CheckMetadataHash with the runtime digest",
            ));
        }
        None => {}
    }
    // 11. Era policy.
    match rebuilt.era {
        EraInfo::Immortal if !inner.policy.allow_immortal => {
            return Err(refuse(
                ErrorCode::EraRejected,
                "immortal transactions are disabled on this server",
            ));
        }
        EraInfo::Immortal => {}
        EraInfo::Mortal { period, .. } => {
            if period > inner.policy.max_era_period {
                return Err(refuse(
                    ErrorCode::EraRejected,
                    format!(
                        "era period {period} exceeds max_era_period {}",
                        inner.policy.max_era_period
                    ),
                ));
            }
            if inner.source.is_online() {
                check_era_anchor(inner, rebuilt.era, period, rebuilt.params.era_block_hash).await?;
            }
        }
    }
    // 12. Call policy.
    let decoded = super::classify_call(&runtime, call.as_bytes()).map_err(|error| {
        tracing::debug!(target: "rtao_remote_signer::audit", error = %error, "call cannot be classified");
        refuse(
            ErrorCode::UndecodableCall,
            "call cannot be decoded for policy",
        )
    })?;
    audit.call = Some(decoded.label());
    let fee = if inner.policy.limits.max_fee.is_some() {
        inner
            .source
            .estimate_fee(call.clone(), account, crypto)
            .await
            .map_err(source_unavailable("fee estimation"))?
    } else {
        None
    };
    let violations = inner.policy.evaluate(&decoded, fee);
    if !violations.is_empty() {
        return Err(Refusal {
            code: ErrorCode::PolicyDenied,
            detail: "the server's policy refused this call".into(),
            violations: Some(violations),
        });
    }
    // 13. Sign server-derived bytes only.
    let context = SigningContext {
        metadata_bytes: runtime.core().metadata_bytes.clone(),
        chain_info,
        transaction_version: runtime.transaction_version(),
        genesis_hash: inner.genesis_hash,
    };
    let purpose = SignPurpose::from(request.purpose);
    let signing = SigningRequest {
        call: call.as_bytes(),
        included_in_extrinsic: &rebuilt.extra,
        included_in_signed_data: &rebuilt.additional,
        payload: &rebuilt.payload,
        payload_json: &rebuilt.payload_json,
        context: &context,
        purpose,
    };
    let signature = signer.sign(&signing).await.map_err(|error| {
        tracing::warn!(target: "rtao_remote_signer::audit", account = %Ss58Address::from(account), error = %error, "backend signer failed");
        refuse(ErrorCode::SigningFailed, "the backend signer failed")
    })?;
    // 14. Self-check.
    let valid = signature.crypto == crypto
        && rtao_core::keys::verify_strict(
            &rebuilt.payload,
            &signature.bytes,
            &account.to_ss58(WIRE_SS58_PREFIX),
            u8::from(crypto),
        )
        .unwrap_or(false);
    if !valid {
        return Err(refuse(
            ErrorCode::SigningFailed,
            "the backend signature did not verify",
        ));
    }
    Ok(SignResponse {
        version: PROTOCOL.into(),
        account: Ss58(account),
        signature: signature.to_bytes().to_vec().into(),
    })
}
