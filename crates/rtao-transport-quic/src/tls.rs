//! TLS 1.3 and QUIC transport configuration with the ring provider.
//!
//! The client accepts any self-signed server certificate, because identity
//! comes from the signed handshake that binds the certificate fingerprint.
//! Unlike btlightning, it verifies the TLS 1.3 `CertificateVerify`
//! signature, so a relay holding only the miner's public certificate cannot
//! complete the TLS handshake. That check is invisible to servers.

use std::sync::Arc;
use std::time::Duration;

use quinn::{IdleTimeout, TransportConfig};
use rustls::DigitallySignedStruct;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls_pki_types::{CertificateDer, ServerName, UnixTime};

use crate::wire::ALPN;

/// TLS or QUIC configuration that cannot be built.
#[derive(Debug, thiserror::Error)]
#[error("TLS configuration error: {0}")]
pub struct TlsError(pub String);

/// The process-independent ring provider every config uses.
pub fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// A server certificate verifier that trusts any end-entity certificate
/// (optionally pinned by fingerprint) but checks the TLS 1.3 handshake
/// signature against it.
#[derive(Debug)]
pub struct FingerprintVerifier {
    provider: Arc<CryptoProvider>,
    pinned: Option<[u8; 32]>,
}

impl FingerprintVerifier {
    /// A verifier that accepts any certificate, or only the certificate
    /// whose BLAKE2b-256 fingerprint is `pinned`.
    pub fn new(pinned: Option<[u8; 32]>) -> Self {
        Self {
            provider: provider(),
            pinned,
        }
    }
}

impl ServerCertVerifier for FingerprintVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if let Some(pin) = self.pinned
            && crate::auth::cert_fingerprint(end_entity) != pin
        {
            return Err(rustls::Error::General(
                "server certificate fingerprint does not match the pin".into(),
            ));
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::PeerIncompatible(
            rustls::PeerIncompatible::Tls12NotOffered,
        ))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        // No fallback on parse failure: skipping this check would let a
        // relay present the miner's certificate without its key.
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Idle timeout and keep-alive shared by both peers.
pub fn transport(idle: Duration, keep_alive: Duration) -> Result<TransportConfig, TlsError> {
    let mut transport = TransportConfig::default();
    let idle = IdleTimeout::try_from(idle).map_err(|e| TlsError(e.to_string()))?;
    transport.max_idle_timeout(Some(idle));
    transport.keep_alive_interval(Some(keep_alive));
    Ok(transport)
}

/// A quinn client configuration offering ALPN `btlightning`.
pub fn client_config(
    pinned: Option<[u8; 32]>,
    transport: TransportConfig,
) -> Result<quinn::ClientConfig, TlsError> {
    let mut tls = rustls::ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| TlsError(e.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(FingerprintVerifier::new(pinned)))
        .with_no_client_auth();
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(tls)
        .map_err(|e| TlsError(e.to_string()))?;
    let mut config = quinn::ClientConfig::new(Arc::new(crypto));
    config.transport_config(Arc::new(transport));
    Ok(config)
}

/// A self-signed certificate, its PKCS#8 key and its fingerprint.
#[cfg(feature = "server")]
pub struct ServerIdentity {
    /// Certificate DER.
    pub cert: CertificateDer<'static>,
    /// Private key DER.
    pub key: rustls_pki_types::PrivateKeyDer<'static>,
    /// BLAKE2b-256 of the certificate DER.
    pub fingerprint: [u8; 32],
}

#[cfg(feature = "server")]
impl ServerIdentity {
    /// Generate a fresh ECDSA P-256 certificate for `localhost`, as
    /// btlightning does on every start.
    pub fn generate() -> Result<Self, TlsError> {
        let key = rcgen::KeyPair::generate().map_err(|e| TlsError(e.to_string()))?;
        let params = rcgen::CertificateParams::new(vec!["localhost".to_owned()])
            .map_err(|e| TlsError(e.to_string()))?;
        let cert = params
            .self_signed(&key)
            .map_err(|e| TlsError(e.to_string()))?;
        let der = cert.der().clone();
        let fingerprint = crate::auth::cert_fingerprint(&der);
        Ok(Self {
            cert: der,
            key: rustls_pki_types::PrivateKeyDer::Pkcs8(key.serialize_der().into()),
            fingerprint,
        })
    }
}

/// A quinn server configuration with a single certificate, ALPN
/// `btlightning`, no client authentication and no unidirectional streams.
#[cfg(feature = "server")]
pub fn server_config(
    identity: &ServerIdentity,
    mut transport: TransportConfig,
    max_bidi_streams: u32,
) -> Result<quinn::ServerConfig, TlsError> {
    let mut tls = rustls::ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| TlsError(e.to_string()))?
        .with_no_client_auth()
        .with_single_cert(vec![identity.cert.clone()], identity.key.clone_key())
        .map_err(|e| TlsError(e.to_string()))?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    transport.max_concurrent_bidi_streams(quinn::VarInt::from_u32(max_bidi_streams));
    transport.max_concurrent_uni_streams(quinn::VarInt::from_u32(0));
    let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(tls)
        .map_err(|e| TlsError(e.to_string()))?;
    let mut config = quinn::ServerConfig::with_crypto(Arc::new(crypto));
    config.transport_config(Arc::new(transport));
    Ok(config)
}

/// The fingerprint of the first certificate the peer presented.
pub fn peer_fingerprint(connection: &quinn::Connection) -> Option<[u8; 32]> {
    let identity = connection.peer_identity()?;
    let certs = identity.downcast::<Vec<CertificateDer<'static>>>().ok()?;
    certs.first().map(|c| crate::auth::cert_fingerprint(c))
}
