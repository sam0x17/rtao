// Derived from RaoFoundation/subtensor sdk/bittensor-core/src/signers/ledger.rs @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Hand-authored APDU protocol fixtures against the patched core's own driver.
//!
//! These scripts are not recorded hardware or emulator transcripts.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::Arc,
    time::Duration,
};

use rtao_core::{
    digest::ChainInfo,
    keys::{CRYPTO_ED25519, Keypair, ss58_from_public, verify},
};
use rtao_signer::{
    LedgerOptions, LedgerSigner, SignPurpose, Signer, SignerPayloadJson, SigningContext,
    SigningRequest,
};
use rtao_testkit::{
    fixtures,
    ledger_emu::{Exchange, ReplayTransport, TcpTransport},
};

fn exchange(request: &[u8], reply: &[u8]) -> Exchange {
    Exchange {
        request: request.to_vec(),
        reply: reply.to_vec(),
    }
}

fn apdu(ins: u8, p1: u8, data: &[u8]) -> Vec<u8> {
    let mut result = vec![0xf9, ins, p1, 0, u8::try_from(data.len()).unwrap()];
    result.extend_from_slice(data);
    result
}

fn path(account: u32, index: u32) -> Vec<u8> {
    [44, 354, account, 0, index]
        .into_iter()
        .flat_map(|word| (word | 0x8000_0000).to_le_bytes())
        .collect()
}

#[test]
fn ledger_replay_uses_fixed_chunk_boundaries_and_complete_signature_reply() {
    let mut init = path(2, 7);
    init.extend_from_slice(&300u16.to_le_bytes());
    let payload = vec![0xab; 300];
    let proof = vec![0xcd; 200];
    let last = [vec![0xab; 50], proof.clone()].concat();
    let signature = [vec![0], vec![0xef; 64]].concat();
    let response = [signature.clone(), vec![0x90, 0]].concat();
    let replay = ReplayTransport::new([
        exchange(&apdu(2, 0, &init), &[0x90, 0]),
        exchange(&apdu(2, 1, &[0xab; 250]), &[0x90, 0]),
        exchange(&apdu(2, 2, &last), &response),
    ])
    .unwrap();
    assert_eq!(
        replay.device().sign(2, 7, &payload, &proof).unwrap(),
        signature
    );
    replay.assert_finished().unwrap();
}

#[test]
fn core_maps_rejection_reason_status_and_malformed_replies() {
    for (reply, expected) in [
        (vec![0x69, 0x86], "the request was rejected on the device"),
        (
            [b"wrong metadata digest".to_vec(), vec![0x69, 0x84]].concat(),
            "device returned 0x6984 (data is invalid): wrong metadata digest",
        ),
        (vec![0x12, 0x34], "device returned 0x1234"),
        (vec![0], "APDU response was too short"),
        (vec![0x90, 0], "unexpected GET_VERSION response length"),
    ] {
        let replay = ReplayTransport::new([exchange(&[0xf9, 0, 0, 0, 0], &reply)]).unwrap();
        assert_eq!(
            replay.device().app_version().err().unwrap().to_string(),
            format!("device error: {expected}")
        );
        replay.assert_finished().unwrap();
    }
}

#[test]
fn complete_addresses_include_path_prefix_and_confirmation_flag() {
    let public = [0x42; 32];
    let address = ss58_from_public(public, 42);
    let reply = [public.to_vec(), address.as_bytes().to_vec(), vec![0x90, 0]].concat();
    for confirm in [false, true] {
        let mut data = path(2, 7);
        data.extend_from_slice(&42u16.to_le_bytes());
        let replay =
            ReplayTransport::new([exchange(&apdu(1, u8::from(confirm), &data), &reply)]).unwrap();
        let actual = replay.device().address(2, 7, 42, confirm).unwrap();
        assert_eq!(actual.public_key, public);
        assert_eq!(actual.ss58_address, address);
        replay.assert_finished().unwrap();
    }
}

#[tokio::test]
async fn ledger_signer_proof_vector_flows_through_core_apdu_driver() {
    let vector: serde_json::Value = serde_json::from_str(include_str!(
        "../../../vendor/upstream/ledger_proof_vector.json"
    ))
    .unwrap();
    let golden =
        fixtures::Golden::from_slice(include_bytes!("../../../vendor/upstream/golden.json"))
            .unwrap();
    let decode = |field: &str| hex::decode(vector[field].as_str().unwrap()).unwrap();
    let call = decode("call_data_hex");
    let extra = decode("included_in_extrinsic_hex");
    let additional = decode("included_in_signed_data_hex");
    let proof = decode("rust_proof_hex");
    let payload = [&*call, &*extra, &*additional].concat();
    let key = Keypair::from_seed(&[7; 32], CRYPTO_ED25519).unwrap();
    let raw_signature = key.sign(&payload).unwrap();
    let public = key.public_key_bytes();
    let address = ss58_from_public(public, 42);
    let mut address_data = path(0, 0);
    address_data.extend_from_slice(&42u16.to_le_bytes());
    let mut init = path(0, 0);
    init.extend_from_slice(&u16::try_from(payload.len()).unwrap().to_le_bytes());
    let mut script = vec![
        exchange(
            &apdu(1, 0, &address_data),
            &[public.to_vec(), address.as_bytes().to_vec(), vec![0x90, 0]].concat(),
        ),
        exchange(&apdu(2, 0, &init), &[0x90, 0]),
    ];
    let message = [&*payload, &*proof].concat();
    let chunks = message.chunks(250).collect::<Vec<_>>();
    for (index, chunk) in chunks.iter().enumerate() {
        let final_chunk = index + 1 == chunks.len();
        let reply = if final_chunk {
            [vec![0], raw_signature.clone(), vec![0x90, 0]].concat()
        } else {
            vec![0x90, 0]
        };
        script.push(exchange(
            &apdu(2, if final_chunk { 2 } else { 1 }, chunk),
            &reply,
        ));
    }
    let replay = ReplayTransport::new(script).unwrap();
    let signer = LedgerSigner::with_backend(Arc::new(replay.device()), LedgerOptions::default())
        .await
        .unwrap();
    let context = SigningContext {
        metadata_bytes: fixtures::metadata_bytes(&golden.metadata.v15_hex).unwrap(),
        chain_info: ChainInfo {
            spec_version: 461,
            spec_name: "node-subtensor".into(),
            base58_prefix: 42,
            decimals: 9,
            token_symbol: "TAO".into(),
        },
        transaction_version: 1,
        genesis_hash: fixtures::decode_fixed_hex(&golden.network.genesis_hash).unwrap(),
    };
    let payload_json = SignerPayloadJson::new();
    let signature = signer
        .sign(&SigningRequest {
            call: &call,
            included_in_extrinsic: &extra,
            included_in_signed_data: &additional,
            payload: &payload,
            payload_json: &payload_json,
            context: &context,
            purpose: SignPurpose::Extrinsic,
        })
        .await
        .unwrap();
    assert_eq!(signature.bytes.as_slice(), raw_signature.as_slice());
    assert!(verify(&payload, &signature.bytes, &address, CRYPTO_ED25519).unwrap());
    replay.assert_finished().unwrap();
}

#[test]
fn speculos_tcp_response_length_excludes_status_word() {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let peer = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut frame = [0; 9];
        stream.read_exact(&mut frame).unwrap();
        assert_eq!(frame, [0, 0, 0, 5, 0xf9, 0, 0, 0, 0]);
        // Fragmentation includes a split status word. The four-byte size says
        // seven data bytes, and the driver must consume two more status bytes.
        for bytes in [
            &[0, 0][..],
            &[0, 7, 0, 0, 100][..],
            &[0, 0, 0, 30, 0x90][..],
            &[0][..],
        ] {
            stream.write_all(bytes).unwrap();
        }
    });
    let device = TcpTransport::connect(address, Duration::from_secs(3))
        .unwrap()
        .into_device();
    assert_eq!(device.app_version().unwrap(), (100, 0, 30));
    peer.join().unwrap();
}

#[test]
fn truncated_tcp_reply_retires_stream_and_public_bind_is_refused() {
    assert!(
        TcpTransport::connect("192.0.2.1:9999".parse().unwrap(), Duration::from_secs(1)).is_err()
    );
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let peer = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut request = [0; 9];
        stream.read_exact(&mut request).unwrap();
        stream.write_all(&[0, 0, 0, 7, 0]).unwrap();
    });
    let device = TcpTransport::connect(address, Duration::from_secs(3))
        .unwrap()
        .into_device();
    assert!(device.app_version().is_err());
    assert!(
        device
            .app_version()
            .err()
            .unwrap()
            .to_string()
            .contains("stream was retired")
    );
    peer.join().unwrap();
}

#[test]
fn excessive_tcp_reply_length_retires_stream_before_allocating() {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let peer = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut request = [0; 9];
        stream.read_exact(&mut request).unwrap();
        stream.write_all(&u32::MAX.to_be_bytes()).unwrap();
    });
    let device = TcpTransport::connect(address, Duration::from_secs(3))
        .unwrap()
        .into_device();
    assert!(device.app_version().is_err());
    assert!(
        device
            .app_version()
            .err()
            .unwrap()
            .to_string()
            .contains("stream was retired")
    );
    peer.join().unwrap();
}

#[test]
fn deadline_retires_a_peer_that_stalls_mid_frame() {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = listener.local_addr().unwrap();
    // Establish the connection before spawning the peer so a connect failure
    // cannot leave a thread waiting in accept().
    let device = TcpTransport::connect(address, Duration::from_millis(100))
        .unwrap()
        .into_device();
    let (release, wait) = std::sync::mpsc::channel();
    let peer = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut request = [0; 9];
        stream.read_exact(&mut request).unwrap();
        stream.write_all(&[0, 0]).unwrap();
        wait.recv_timeout(Duration::from_secs(3)).is_ok()
    });
    let started = std::time::Instant::now();
    let outcome = device.app_version();
    let elapsed = started.elapsed();
    let released = release.send(()).is_ok();
    let peer_held_open = peer.join();
    let retirement = device.app_version();

    // Cleanup precedes assertions. A missing client deadline must fail even
    // when the peer's fallback eventually closes the socket and produces EOF.
    assert!(
        released,
        "the peer fallback closed before the client completed"
    );
    assert!(
        peer_held_open.unwrap(),
        "the peer must remain open until client completion"
    );
    assert!(outcome.is_err());
    assert!(
        elapsed < Duration::from_secs(2),
        "100 ms APDU deadline took {elapsed:?}"
    );
    assert!(
        retirement
            .err()
            .unwrap()
            .to_string()
            .contains("stream was retired")
    );
}
