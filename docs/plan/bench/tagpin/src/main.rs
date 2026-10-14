fn main() {
    let kp = bittensor_core::keys::Keypair::from_uri("//Alice", bittensor_core::keys::CRYPTO_SR25519).unwrap();
    println!("{}", kp.ss58_address());
}
