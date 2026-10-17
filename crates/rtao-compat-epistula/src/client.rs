//! Sign materialized reqwest requests.
//!
//! Every call signs afresh with a new timestamp and uuid, so a retry never
//! resends a signature a replay-protecting receiver already accepted. Disable
//! reqwest's own retries and redirects on the supplied client.
//!
//! # Apex
//!
//! SN1 Apex clients do not hash the wire body. They sign
//! `json.dumps(data, sort_keys=True)` of the JSON body, of `{}` when there is
//! no body, and of the query parameters (before `None` values are dropped)
//! for a `GET`. [`sign_reqwest`](crate::client::sign_reqwest) hashes the
//! wire body, so it matches Apex only for a POST whose body is exactly
//! [`canonical_json`](crate::apex::canonical_json) of the data. For a `GET`
//! or an empty body, pass the Apex digest explicitly with
//! [`sign_reqwest_with_digest`](crate::client::sign_reqwest_with_digest) or
//! [`execute_with_digest`](crate::client::EpistulaClient::execute_with_digest),
//! for example `BodyDigest::Raw(&canonical_json(&json!({})))`.

use std::{fmt, sync::Arc};

use rtao_btauth::{Clock, SystemClock};
use rtao_signer::Signer;
use sha2::Digest;

use crate::{BodyDigest, Dialect, EpistulaError, SignOptions, now_ms, sign_headers};

/// Sign `req` in place, hashing its materialized body.
///
/// A missing body signs the empty string. A streaming body cannot be hashed
/// before it is sent and is refused. See the module documentation for Apex.
pub async fn sign_reqwest(
    req: &mut reqwest::Request,
    signer: &dyn Signer,
    dialect: &Dialect,
    signed_for: Option<&str>,
    clock: &dyn Clock,
) -> Result<(), EpistulaError> {
    let body: &[u8] = match req.body() {
        None => &[],
        Some(body) => body.as_bytes().ok_or(EpistulaError::Usage(
            "streaming request bodies cannot be signed",
        ))?,
    };
    let digest = sha2::Sha256::digest(body).into();
    sign_reqwest_with_digest(
        req,
        signer,
        dialect,
        signed_for,
        BodyDigest::Sha256(digest),
        clock,
    )
    .await
}

/// Sign `req` in place over an explicit digest input instead of its body.
///
/// The receiver must hash the same bytes; this is for senders, such as Apex
/// clients, that sign something other than the wire body.
pub async fn sign_reqwest_with_digest(
    req: &mut reqwest::Request,
    signer: &dyn Signer,
    dialect: &Dialect,
    signed_for: Option<&str>,
    digest: BodyDigest<'_>,
    clock: &dyn Clock,
) -> Result<(), EpistulaError> {
    let headers = sign_headers(
        signer,
        dialect,
        SignOptions {
            digest,
            signed_for,
            now_ms: now_ms(clock)?,
            uuid: None,
        },
    )
    .await?;
    headers.apply(req.headers_mut())
}

/// A signing or transport failure.
#[derive(Debug)]
pub enum ClientError {
    /// The request could not be signed.
    Epistula(EpistulaError),
    /// The HTTP request failed.
    Http(reqwest::Error),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Epistula(error) => error.fmt(f),
            Self::Http(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ClientError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Epistula(error) => Some(error),
            Self::Http(error) => Some(error),
        }
    }
}

/// A thin client that signs every request it executes.
#[derive(Clone)]
pub struct EpistulaClient {
    http: reqwest::Client,
    signer: Arc<dyn Signer>,
    dialect: Dialect,
    clock: Arc<dyn Clock>,
}

impl fmt::Debug for EpistulaClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EpistulaClient")
            .field("signer", &self.signer.account())
            .field("dialect", &self.dialect)
            .finish_non_exhaustive()
    }
}

impl EpistulaClient {
    /// Sign with `signer` under `dialect`, sending through `http`.
    pub fn new(http: reqwest::Client, signer: Arc<dyn Signer>, dialect: Dialect) -> Self {
        Self {
            http,
            signer,
            dialect,
            clock: Arc::new(SystemClock::default()),
        }
    }

    /// Supply the signing clock, usually for tests.
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// The underlying HTTP client, for building requests.
    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// Sign and execute one request. Call again to retry; each call re-signs.
    pub async fn execute(
        &self,
        mut req: reqwest::Request,
        signed_for: Option<&str>,
    ) -> Result<reqwest::Response, ClientError> {
        sign_reqwest(
            &mut req,
            self.signer.as_ref(),
            &self.dialect,
            signed_for,
            self.clock.as_ref(),
        )
        .await
        .map_err(ClientError::Epistula)?;
        self.http.execute(req).await.map_err(ClientError::Http)
    }

    /// Sign over an explicit digest input and execute one request.
    ///
    /// Use this for Apex `GET` and empty-body requests (see the module
    /// documentation). Call again to retry; each call re-signs.
    pub async fn execute_with_digest(
        &self,
        mut req: reqwest::Request,
        signed_for: Option<&str>,
        digest: BodyDigest<'_>,
    ) -> Result<reqwest::Response, ClientError> {
        sign_reqwest_with_digest(
            &mut req,
            self.signer.as_ref(),
            &self.dialect,
            signed_for,
            digest,
            self.clock.as_ref(),
        )
        .await
        .map_err(ClientError::Epistula)?;
        self.http.execute(req).await.map_err(ClientError::Http)
    }
}
