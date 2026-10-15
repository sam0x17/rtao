// Derived from RaoFoundation/subtensor sdk/bittensor-core/src/timelock/mod.rs @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! The core's encryption algorithm with fixed test entropy in both BLS builds.

use ark_serialize::{CanonicalDeserialize, CanonicalSerialize};
use rand_chacha::ChaCha20Rng;
use rand_core::SeedableRng;
use sha2::Digest;
use tle::{
    curves::drand::TinyBLS381, ibe::fullident::Identity,
    stream_ciphers::AESGCMStreamCipherProvider, tlock::tle,
};
use w3f_bls::EngineBLS;

/// Fixed ephemeral key for reproducible public test vectors, never production use.
pub const EPHEMERAL_KEY: [u8; 32] = [0x17; 32];
/// Fixed ChaCha20 seed used by both implementations.
pub const RNG_SEED: [u8; 32] = [0x42; 32];
/// Quicknet public key authenticated by the pinned core constants.
pub const QUICKNET_PUBLIC_KEY: &str = "83cf0f2896adee7eb8b5f01fcad3912212c437e0073e911fb90022d3e760183c8c4b450b6a0a6c3ac6a5776a2d1064510d1fec758c921cc22b0e17e63aaf4bcb5ed66304de9cf809bd274ca73bab4af5a6e9c76a4bc09e76eae8991ef5ece45a";

/// Compress one TLE weight payload using the exact pinned core algorithm.
pub fn encrypt(payload: &[u8], round: u64) -> Result<Vec<u8>, String> {
    if round == 0 {
        return Err("Quicknet rounds begin at one".into());
    }
    let public_bytes = hex::decode(QUICKNET_PUBLIC_KEY).map_err(|e| e.to_string())?;
    let public =
        <TinyBLS381 as EngineBLS>::PublicKeyGroup::deserialize_compressed(public_bytes.as_slice())
            .map_err(|e| e.to_string())?;
    let identity = Identity::new(
        b"",
        vec![sha2::Sha256::digest(round.to_be_bytes()).to_vec()],
    );
    let ciphertext = tle::<TinyBLS381, AESGCMStreamCipherProvider, ChaCha20Rng>(
        public,
        EPHEMERAL_KEY,
        payload,
        identity,
        ChaCha20Rng::from_seed(RNG_SEED),
    )
    .map_err(|e| format!("encryption failed: {e:?}"))?;
    let mut compressed = Vec::new();
    ciphertext
        .serialize_compressed(&mut compressed)
        .map_err(|e| e.to_string())?;
    Ok(compressed)
}
