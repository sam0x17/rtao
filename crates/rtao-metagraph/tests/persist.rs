//! Raw response round trips and offline failure behavior of RTMG version 1.
#![cfg(feature = "persist")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use parity_scale_codec::{Decode, Encode};
use rtao_client::{Client, ClientConfig, Network, RuntimeCache};
use rtao_core::{codec::decode::Cursor, runtime::Runtime};
use rtao_metagraph::{
    Metagraph, MetagraphOptions, Preset, SnapshotInfo, SubnetsMetagraphExt,
    persist::{self, PersistError},
};
use rtao_primitives::{H256, MechId, NetUid};
use rtao_reads::Namespaces;
use rtao_testkit::rpc::RpcClient;
use rtao_testkit::{FakeChain, FakeState};
use serde_json::json;

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let root = std::env::var_os("RTAO_TEMP_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let path = root.join("metagraph-persist-tests").join(format!(
            "{}-{}",
            std::process::id(),
            NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn file(&self) -> PathBuf {
        self.0.join("graph.rtmg")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // This exact, process-owned directory holds only files written by this
        // test; no subprocess, configuration, wallet or virtualenv uses it.
        if let Ok(entries) = fs::read_dir(&self.0) {
            for entry in entries.flatten() {
                assert!(!entry.file_type().unwrap().is_symlink());
            }
        }
        fs::remove_dir_all(&self.0).unwrap();
    }
}

// Independent test description of the published SCALE header, including stable
// source indices. Mutated files below exercise the decoder's boundary.
#[derive(Debug, Encode, Decode)]
struct FileHeader {
    genesis: [u8; 32],
    block_hash: [u8; 32],
    block_number: u64,
    spec_version: u32,
    netuid: u16,
    mechid: u8,
    preset: FilePreset,
}

#[derive(Debug, PartialEq, Eq, Encode, Decode)]
enum FilePreset {
    #[codec(index = 0)]
    FullSubnet,
    #[codec(index = 1)]
    FullMechanism,
    #[codec(index = 2)]
    Selective(Vec<u8>),
}

fn file_parts(bytes: &[u8]) -> (FileHeader, &[u8]) {
    assert_eq!(&bytes[..5], b"RTMG\x01");
    let mut raw = &bytes[5..];
    let header = FileHeader::decode(&mut raw).unwrap();
    (header, raw)
}

fn encode_file(header: &FileHeader, raw: &[u8]) -> Vec<u8> {
    let mut bytes = b"RTMG\x01".to_vec();
    bytes.extend(header.encode());
    bytes.extend(raw);
    bytes
}

fn options() -> MetagraphOptions {
    MetagraphOptions {
        commitments: false,
        ..MetagraphOptions::default()
    }
}

async fn setup(scratch: &Scratch) -> (FakeChain, Client, RuntimeCache) {
    let runtime = Arc::new(
        Runtime::parse(
            include_bytes!("../../../vendor/metadata/finney/478.v15.scale"),
            478,
            1,
            42,
        )
        .unwrap(),
    );
    let method = runtime
        .apis
        .iter()
        .find(|api| api.name == "SubnetInfoRuntimeApi")
        .unwrap()
        .methods
        .iter()
        .find(|method| method.name == "get_metagraph")
        .unwrap();
    let full = include_bytes!("../../../vendor/bench/finney-1.scale");
    let mut cursor = Cursor::new(full);
    let value = runtime.decode_id(method.output, &mut cursor).unwrap();
    let mut mechanism = rtao_core::codec::value::to_corpus_json(&value);
    mechanism["netuid"] = json!(8193); // subnet 1, mechanism 2
    let mut state = FakeState::new(runtime);
    state.seed_runtime_encoded(
        "SubnetInfoRuntimeApi_get_metagraph",
        vec![1, 0],
        full.to_vec(),
    );
    state
        .seed_runtime("SubnetInfoRuntimeApi", "get_mechagraph", mechanism)
        .unwrap();
    state
        .seed_runtime(
            "SubnetInfoRuntimeApi",
            "get_selective_mechagraph",
            json!({
                "netuid": 1,
                "hotkeys": [format!("0x{}", hex::encode([11; 32])), format!("0x{}", hex::encode([12; 32]))],
                "validator_permit": [false, true],
                "total_stake": [7, 8],
                "validators": [1]
            }),
        )
        .unwrap();
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None).unwrap()).unwrap();
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config)
        .await
        .unwrap();
    let cache = RuntimeCache::new(
        client.rpc().clone(),
        client.genesis(),
        42,
        Some(scratch.0.join("runtime-cache")),
    );
    cache.runtime_at(None).await.unwrap();
    (chain, client, cache)
}

#[tokio::test]
async fn full_subnet_retains_exact_response_and_loads_from_memory_without_rpc() {
    let scratch = Scratch::new();
    let (chain, client, cache) = setup(&scratch).await;
    let graph = client
        .subnets()
        .metagraph_with(NetUid(1), options())
        .await
        .unwrap()
        .unwrap();
    persist::save(&graph, scratch.file()).await.unwrap();
    let bytes = fs::read(scratch.file()).unwrap();
    let (header, raw) = file_parts(&bytes);
    assert_eq!(header.preset, FilePreset::FullSubnet);
    assert_eq!(header.genesis, client.genesis().0);
    assert_eq!(header.block_hash, graph.at.hash.0);
    assert_eq!(header.block_number, graph.at.number);
    assert_eq!(header.spec_version, 478);
    assert_eq!(raw, include_bytes!("../../../vendor/bench/finney-1.scale"));
    client.close().await;
    let requests = chain.requests().len();
    let loaded = persist::load(scratch.file(), &cache).await.unwrap();
    assert_eq!(chain.requests().len(), requests);
    assert_eq!(loaded.at, graph.at);
    assert_eq!(loaded.header, graph.header);
    assert_eq!(
        loaded.runtime_value().unwrap(),
        graph.runtime_value().unwrap()
    );
    assert_eq!(loaded.raw(), graph.raw());
    assert!(loaded.commitments.is_none());
    assert!(loaded.unregistered_commitments.is_none());
}

#[tokio::test]
async fn full_mechanism_uses_original_api_and_resaves_identically() {
    let scratch = Scratch::new();
    let (chain, client, cache) = setup(&scratch).await;
    let graph = client
        .subnets()
        .mechagraph_with(NetUid(1), MechId(2), options())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(graph.header.mechid, MechId(2));
    persist::save(&graph, scratch.file()).await.unwrap();
    let original = fs::read(scratch.file()).unwrap();
    assert_eq!(file_parts(&original).0.preset, FilePreset::FullMechanism);
    client.close().await;
    let requests = chain.requests().len();
    let loaded = persist::load(scratch.file(), &cache).await.unwrap();
    assert_eq!(loaded.header.mechid, MechId(2));
    assert_eq!(loaded.raw(), graph.raw());
    assert_eq!(
        loaded.runtime_value().unwrap(),
        graph.runtime_value().unwrap()
    );
    let second = scratch.0.join("resaved.rtmg");
    persist::save(&loaded, &second).await.unwrap();
    assert_eq!(fs::read(second).unwrap(), original);
    assert_eq!(chain.requests().len(), requests);
}

#[tokio::test]
async fn selective_miner_preserves_none_fields_and_loads_from_shared_disk_only() {
    let scratch = Scratch::new();
    let (chain, client, cache) = setup(&scratch).await;
    let fields = Preset::Miner.fields().unwrap();
    let graph = client
        .subnets()
        .selective_metagraph_with(NetUid(1), MechId(3), &fields, options())
        .await
        .unwrap()
        .unwrap();
    persist::save(&graph, scratch.file()).await.unwrap();
    let original = fs::read(scratch.file()).unwrap();
    let (header, raw) = file_parts(&original);
    assert_eq!(header.preset, FilePreset::Selective(vec![52, 57, 69, 72]));
    assert_eq!(raw, graph.raw().as_ref());
    drop(cache);
    client.close().await;
    let offline = RuntimeCache::new(
        client.rpc().clone(),
        client.genesis(),
        42,
        Some(scratch.0.join("runtime-cache")),
    );
    let requests = chain.requests().len();
    let loaded = persist::load(scratch.file(), &offline).await.unwrap();
    assert_eq!(chain.requests().len(), requests);
    assert_eq!(loaded.header, graph.header);
    assert_eq!(loaded.at, graph.at);
    assert_eq!(loaded.raw(), graph.raw());
    assert_eq!(
        loaded.runtime_value().unwrap(),
        graph.runtime_value().unwrap()
    );
    assert_eq!(loaded.to_json().unwrap(), graph.to_json().unwrap());
    assert_eq!(loaded.header.mechid, MechId(3));
    let second = scratch.0.join("selective-resaved.rtmg");
    persist::save(&loaded, &second).await.unwrap();
    assert_eq!(fs::read(second).unwrap(), original);
}

#[tokio::test]
async fn malformed_files_are_rejected_unchanged_without_rpc() {
    let scratch = Scratch::new();
    let (chain, client, cache) = setup(&scratch).await;
    let graph = client
        .subnets()
        .metagraph_with(NetUid(1), options())
        .await
        .unwrap()
        .unwrap();
    persist::save(&graph, scratch.file()).await.unwrap();
    let original = fs::read(scratch.file()).unwrap();
    let mut invalid = Vec::new();
    let mut magic = original.clone();
    magic[0] = b'P';
    invalid.push(magic);
    let mut version = original.clone();
    version[4] = 2;
    invalid.push(version);
    invalid.push(b"RTMG".to_vec());
    invalid.push(original[..30].to_vec());
    let (mut header, raw) = file_parts(&original);
    invalid.push(encode_file(&header, &[]));
    invalid.push(encode_file(&header, &[0])); // Option::None
    invalid.push(encode_file(&header, &[1, 255])); // incomplete Some
    let mut trailing = raw.to_vec();
    trailing.push(0);
    invalid.push(encode_file(&header, &trailing));
    header.netuid = 4096;
    invalid.push(encode_file(&header, raw));
    header.netuid = 2; // valid ID but wrong raw subnet
    invalid.push(encode_file(&header, raw));
    header.netuid = 1;
    header.mechid = 16;
    invalid.push(encode_file(&header, raw));
    header.mechid = 1; // valid ID but wrong raw mechanism
    invalid.push(encode_file(&header, raw));
    header.mechid = 0;
    header.preset = FilePreset::Selective(vec![255]);
    invalid.push(encode_file(&header, raw));
    let mut unknown_source = original.clone();
    unknown_source[84] = 255; // 5-byte prefix + 79 fixed header bytes
    invalid.push(unknown_source);
    // A bogus compact length must fail before allocating a claimed huge field
    // vector. The actual file is shorter than that claim.
    let mut oversized_fields = original[..84].to_vec();
    oversized_fields.push(2);
    oversized_fields.extend(parity_scale_codec::Compact(u32::MAX).encode());
    invalid.push(oversized_fields);
    client.close().await;
    let requests = chain.requests().len();
    for bytes in invalid {
        fs::write(scratch.file(), &bytes).unwrap();
        assert!(persist::load(scratch.file(), &cache).await.is_err());
        assert_eq!(fs::read(scratch.file()).unwrap(), bytes);
    }
    header.preset = FilePreset::Selective(vec![255]);
    fs::write(scratch.file(), encode_file(&header, raw)).unwrap();
    assert!(matches!(
        persist::load(scratch.file(), &cache).await,
        Err(PersistError::InvalidFormat(detail)) if detail == "unknown selective metagraph field"
    ));
    assert_eq!(chain.requests().len(), requests);
}

#[tokio::test]
async fn wrong_genesis_and_missing_spec_fail_offline_with_specific_errors() {
    let scratch = Scratch::new();
    let (chain, client, cache) = setup(&scratch).await;
    let graph = client
        .subnets()
        .metagraph_with(NetUid(1), options())
        .await
        .unwrap()
        .unwrap();
    persist::save(&graph, scratch.file()).await.unwrap();
    let original = fs::read(scratch.file()).unwrap();
    let wrong_chain = RuntimeCache::new(client.rpc().clone(), H256([97; 32]), 42, None);
    let missing = RuntimeCache::new(client.rpc().clone(), client.genesis(), 42, None);
    client.close().await;
    let requests = chain.requests().len();
    assert!(matches!(
        persist::load(scratch.file(), &wrong_chain).await,
        Err(PersistError::GenesisMismatch { .. })
    ));
    assert!(matches!(
        persist::load(scratch.file(), &missing).await,
        Err(PersistError::Runtime(_))
    ));
    let (mut header, raw) = file_parts(&original);
    header.spec_version = 999_999;
    fs::write(scratch.file(), encode_file(&header, raw)).unwrap();
    assert!(matches!(
        persist::load(scratch.file(), &cache).await,
        Err(PersistError::Runtime(_))
    ));
    assert_eq!(chain.requests().len(), requests);
}

#[tokio::test]
async fn failed_save_preserves_destination_and_leaves_no_temporary_file() {
    let scratch = Scratch::new();
    let (chain, client, _) = setup(&scratch).await;
    let mut graph = client
        .subnets()
        .metagraph_with(NetUid(1), options())
        .await
        .unwrap()
        .unwrap();
    fs::write(scratch.file(), b"existing snapshot").unwrap();
    let synthetic = Metagraph::from_dynamic(
        NetUid(1),
        graph.runtime_value().unwrap(),
        SnapshotInfo::default(),
    )
    .unwrap();
    assert!(matches!(
        persist::save(&synthetic, scratch.file()).await,
        Err(PersistError::MissingProvenance)
    ));
    graph.at.number += 1;
    assert!(matches!(
        persist::save(&graph, scratch.file()).await,
        Err(PersistError::InvalidFormat(_))
    ));
    assert_eq!(fs::read(scratch.file()).unwrap(), b"existing snapshot");
    graph.at.number -= 1;
    let destination = scratch.0.join("directory");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("marker"), b"keep").unwrap();
    assert!(matches!(
        persist::save(&graph, &destination).await,
        Err(PersistError::Io(_))
    ));
    assert_eq!(fs::read(destination.join("marker")).unwrap(), b"keep");
    assert!(fs::read_dir(&scratch.0).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".rtao-metagraph-")
    }));
    chain.mutate_state(|state| {
        state
            .seed_runtime(
                "SubnetInfoRuntimeApi",
                "get_selective_mechagraph",
                json!({"netuid":0}),
            )
            .unwrap();
    });
    let wrong_subnet = client
        .subnets()
        .selective_metagraph_with(
            NetUid(1),
            MechId(0),
            &Preset::Miner.fields().unwrap(),
            options(),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        persist::save(&wrong_subnet, scratch.file()).await,
        Err(PersistError::InvalidFormat(_))
    ));
    assert_eq!(fs::read(scratch.file()).unwrap(), b"existing snapshot");
    client.close().await;
}

#[tokio::test]
async fn concurrent_atomic_replacements_publish_a_complete_file() {
    let scratch = Scratch::new();
    let (chain, client, cache) = setup(&scratch).await;
    let full = client
        .subnets()
        .metagraph_with(NetUid(1), options())
        .await
        .unwrap()
        .unwrap();
    let mechanism = client
        .subnets()
        .mechagraph_with(NetUid(1), MechId(2), options())
        .await
        .unwrap()
        .unwrap();
    let path = scratch.file();
    let (left, right) = tokio::join!(
        persist::save(&full, &path),
        persist::save(&mechanism, &path)
    );
    left.unwrap();
    right.unwrap();
    client.close().await;
    let requests = chain.requests().len();
    let loaded = persist::load(&path, &cache).await.unwrap();
    assert!(loaded.raw() == full.raw() || loaded.raw() == mechanism.raw());
    assert_eq!(chain.requests().len(), requests);
    assert!(fs::read_dir(&scratch.0).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".rtao-metagraph-")
    }));
}
