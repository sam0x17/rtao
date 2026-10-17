// Derived from BitMind-AI/bitmind-subnet gas/protocol/epistula.py @ 5fd96727b8f41d8376348f6dd8a11cd5069bce45 (MIT); modified by rtao.
// Derived from macrocosm-os/apex shared/common/src/common/utils/epistula.py @ bd71ceb070cd72b4bd80562e96884b66e86d908a (MIT); modified by rtao.
//! Header signing for every dialect.

use http::{HeaderMap, HeaderName, HeaderValue};
use rtao_primitives::CryptoType;
use rtao_signer::Signer;

use crate::{
    BodyDigest, Dialect, EpistulaError, MessageForm, SecretEmit, SecretSubject, VERSION,
    VersionRule, header, request_message, secret_message, signature_text, verify::valid_uuid,
};

/// Signed Epistula headers, in a fixed emission order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EpistulaHeaders {
    /// `Epistula-Version`, absent for the Apex dialect.
    pub version: Option<&'static str>,
    /// `Epistula-Timestamp`.
    pub timestamp: String,
    /// `Epistula-Uuid`, absent for the Apex dialect.
    pub uuid: Option<String>,
    /// `Epistula-Signed-By`.
    pub signed_by: String,
    /// `Epistula-Signed-For`, present for a bound request.
    pub signed_for: Option<String>,
    /// `Epistula-Request-Signature`.
    pub request_signature: String,
    /// `Epistula-Secret-Signature-0..2`.
    pub secret: Option<[String; 3]>,
    /// Apex `(X-Spec-Version, X-Request-Id)`.
    pub apex: Option<(String, String)>,
}

impl EpistulaHeaders {
    /// Header pairs in emission order: version, timestamp, uuid, sender,
    /// receiver, request signature, secret signatures, then Apex extras.
    pub fn iter(&self) -> impl Iterator<Item = (&'static str, &str)> {
        let mut pairs = Vec::with_capacity(11);
        if let Some(version) = self.version {
            pairs.push((header::VERSION, version));
        }
        pairs.push((header::TIMESTAMP, self.timestamp.as_str()));
        if let Some(uuid) = &self.uuid {
            pairs.push((header::UUID, uuid.as_str()));
        }
        pairs.push((header::SIGNED_BY, self.signed_by.as_str()));
        if let Some(receiver) = &self.signed_for {
            pairs.push((header::SIGNED_FOR, receiver.as_str()));
        }
        pairs.push((header::REQUEST_SIGNATURE, self.request_signature.as_str()));
        if let Some(secret) = &self.secret {
            for (name, value) in header::SECRET_SIGNATURE.iter().zip(secret) {
                pairs.push((name, value.as_str()));
            }
        }
        if let Some((spec_version, request_id)) = &self.apex {
            pairs.push((header::APEX_SPEC_VERSION, spec_version.as_str()));
            pairs.push((header::APEX_REQUEST_ID, request_id.as_str()));
        }
        pairs.into_iter()
    }

    /// Insert every header after removing all existing `Epistula-*` headers.
    ///
    /// Re-signing a request therefore never leaves a stale `Signed-For`, uuid
    /// or secret signature behind. Apex extras replace values of the same
    /// names; other headers are kept.
    pub fn apply(&self, map: &mut HeaderMap) -> Result<(), EpistulaError> {
        let mut pairs = Vec::with_capacity(11);
        for (name, value) in self.iter() {
            let name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| EpistulaError::Usage("invalid Epistula header name"))?;
            let value = HeaderValue::from_str(value).map_err(|_| {
                EpistulaError::Usage("Epistula header value is not valid HTTP text")
            })?;
            pairs.push((name, value));
        }
        let stale: Vec<HeaderName> = map
            .keys()
            .filter(|name| name.as_str().starts_with("epistula-"))
            .cloned()
            .collect();
        for name in stale {
            map.remove(name);
        }
        for (name, value) in pairs {
            map.insert(name, value);
        }
        Ok(())
    }

    /// Build a header map holding only these headers.
    pub fn to_header_map(&self) -> Result<HeaderMap, EpistulaError> {
        let mut map = HeaderMap::new();
        self.apply(&mut map)?;
        Ok(map)
    }
}

/// Inputs to one signing attempt.
#[derive(Clone, Copy, Debug)]
pub struct SignOptions<'a> {
    /// Digest input, normally the exact wire body.
    pub digest: BodyDigest<'a>,
    /// Receiver SS58 address; `None` signs an unbound request.
    pub signed_for: Option<&'a str>,
    /// Signing time in Unix milliseconds.
    pub now_ms: u64,
    /// Fixed uuid (and Apex request id) for tests; `None` generates a uuid4.
    pub uuid: Option<&'a str>,
}

impl<'a> SignOptions<'a> {
    /// Sign `body` at `now_ms` with a fresh uuid and no receiver.
    pub fn new(body: &'a [u8], now_ms: u64) -> Self {
        Self {
            digest: BodyDigest::Raw(body),
            signed_for: None,
            now_ms,
            uuid: None,
        }
    }

    /// Bind the request to a receiver SS58 address.
    pub fn signed_for(mut self, receiver: &'a str) -> Self {
        self.signed_for = Some(receiver);
        self
    }
}

/// Generate a random lowercase RFC 4122 version-4 uuid.
pub fn uuid4() -> Result<String, EpistulaError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|_| EpistulaError::Usage("OS randomness is unavailable"))?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = hex::encode(bytes);
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    ))
}

/// Sign Epistula headers for one attempt.
///
/// Call this again for every retry: a receiver that rejects replays refuses a
/// resent uuid or signature. The signer must support raw sr25519 signing;
/// hardware signers refuse it. One raw signature is made for the request and
/// three more when the dialect emits secret signatures.
pub async fn sign_headers(
    signer: &dyn Signer,
    dialect: &Dialect,
    opts: SignOptions<'_>,
) -> Result<EpistulaHeaders, EpistulaError> {
    if signer.crypto_type() != CryptoType::Sr25519 {
        return Err(EpistulaError::UnsupportedCrypto);
    }
    let receiver = match opts.signed_for {
        Some("") => {
            return Err(EpistulaError::Usage(
                "signed_for must be a receiver address; use None for an unbound request",
            ));
        }
        None if dialect.require_bound_signing => {
            return Err(EpistulaError::Usage(
                "this dialect requires a Signed-For receiver",
            ));
        }
        receiver => receiver,
    };
    if let Some(uuid) = opts.uuid
        && !valid_uuid(uuid)
    {
        return Err(EpistulaError::Usage(
            "uuid must be 1 to 128 visible ASCII characters",
        ));
    }
    let sender =
        rtao_core::keys::ss58_from_public(signer.account().0, rtao_core::keys::DEFAULT_SS58_FORMAT);
    let timestamp = opts.now_ms.to_string();
    let uuid = match dialect.message {
        MessageForm::WithUuid => Some(match opts.uuid {
            Some(uuid) => uuid.to_owned(),
            None => uuid4()?,
        }),
        MessageForm::NoUuid => None,
    };
    let message = request_message(
        dialect.message,
        &opts.digest.hex(),
        uuid.as_deref(),
        &timestamp,
        receiver.unwrap_or_default(),
    );
    let request_signature = sign_text(signer, &message).await?;
    let emit_secret = match dialect.secret.emit {
        SecretEmit::Never => false,
        SecretEmit::WhenBound => receiver.is_some(),
        SecretEmit::Always => true,
    };
    let secret = if emit_secret {
        let subject = match dialect.secret.subject {
            SecretSubject::Receiver => receiver.ok_or(EpistulaError::Usage(
                "receiver-subject secret signatures require Signed-For",
            ))?,
            SecretSubject::Sender => sender.as_str(),
        };
        let mut signatures: [String; 3] = Default::default();
        for (k, slot) in (0u8..).zip(signatures.iter_mut()) {
            let message = secret_message(dialect.secret.interval, opts.now_ms, k, subject)?;
            *slot = sign_text(signer, &message).await?;
        }
        Some(signatures)
    } else {
        None
    };
    let apex = match &dialect.apex_extras {
        Some(extras) => Some((
            extras.spec_version.to_string(),
            match opts.uuid {
                Some(uuid) => uuid.to_owned(),
                None => uuid4()?,
            },
        )),
        None => None,
    };
    Ok(EpistulaHeaders {
        version: match dialect.version {
            VersionRule::EmitRequire | VersionRule::EmitIgnore => Some(VERSION),
            VersionRule::Omit => None,
        },
        timestamp,
        uuid,
        signed_by: sender,
        signed_for: receiver.map(str::to_owned),
        request_signature,
        secret,
        apex,
    })
}

async fn sign_text(signer: &dyn Signer, message: &str) -> Result<String, EpistulaError> {
    // btwallet's Python `Keypair.sign(str)` hex-decodes text starting with
    // `0x`. No Epistula message can: each starts with hex digest or digits.
    debug_assert!(!message.starts_with("0x"));
    signer
        .sign_raw(message.as_bytes())
        .await
        .map(|signature| signature_text(&signature))
        .map_err(EpistulaError::Signing)
}
