// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_facades.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Compile-time correspondence for the pinned Python facade's async surface.
//!
//! `xtask` compares every public Python coroutine/async-generator signature to
//! these tables. The compiler checks each native blocking signature without
//! opening a connection or executing an operation. The blocking implementation
//! delegates to the native async Client/View/Executor and is compiled with it.
//!
//! Python descriptor tuples split into pallet/item or api/method; owned values
//! cross the worker boundary; historical reads use `at`; typed options replace
//! wallet/keyword arguments. Native results, references and error types remain
//! explicit below. Zero-argument Python properties are Rust methods. `blocks`
//! returns an Iterator, and optional PoW exposes the native solver configuration.
//! Native-only helpers are outside the pinned Python reflection assertion.

#![cfg(feature = "blocking")]

use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use rtao::blocking::{Blocks, EpochEvent, Error, SyncClient, SyncMultisig, SyncSnapshot};
use rtao_client::{BlockHeader, BlockInfo, BlockRef, CallBytes, Finality, UnsignedExtrinsic};
use rtao_core::{DynValue, codec::Value};
use rtao_intents::transfer::Transfer;
use rtao_primitives::{AccountId, Balance, CryptoType, NetUid, Tao};
use rtao_reads::ReadName;
use rtao_signer::{Coldkey, KeypairSigner, MultiSignature};
use rtao_tx::{
    ExecuteOptions, ExtrinsicResult, IntentPreflight, Plan, PlanOptions, PrepareOptions,
    ShieldOptions,
};

type FacadeResult<T> = Result<T, Error>;
type TestResult = Result<(), Box<dyn std::error::Error>>;
type MapRows = Vec<(Vec<DynValue>, DynValue)>;
type ParamSets = Vec<Vec<Value>>;
type JsonArguments = serde_json::Map<String, serde_json::Value>;
type DynamicKeys = Arc<dyn rtao_signer::DynKeySource>;

// The source signature and the typed witness share one row. Keep the native
// method's name equal to the Python name; xtask rejects omissions/duplicates.
macro_rules! facade_contract {
    ($test:ident; $( $(#[$attr:meta])* $name:literal [$($param:literal),*] => $method:path : $signature:ty; )*) => {
        #[test]
        fn $test() {
            $( $(#[$attr])* let _: $signature = $method; )*
        }
    };
}

// pyt: unit/test_facades.py::test_sync_facade_mirrors_async_surface[client]
facade_contract! {
    client_contract;
    "query" ["item", "params", "block"] => SyncClient::query : fn(&SyncClient, String, String, Vec<Value>) -> FacadeResult<DynValue>;
    "query_map" ["item", "params", "block"] => SyncClient::query_map : fn(&SyncClient, String, String, Vec<Value>) -> FacadeResult<MapRows>;
    "query_batch" ["item", "param_sets", "block"] => SyncClient::query_batch : fn(&SyncClient, String, String, ParamSets) -> FacadeResult<Vec<DynValue>>;
    "runtime" ["method", "params", "block"] => SyncClient::runtime : fn(&SyncClient, String, String, Vec<Value>) -> FacadeResult<DynValue>;
    "constant" ["item"] => SyncClient::constant : fn(&SyncClient, String, String) -> FacadeResult<DynValue>;
    "decode_scale" ["type_string", "data"] => SyncClient::decode_scale : fn(&SyncClient, String, Vec<u8>) -> FacadeResult<DynValue>;
    "read" ["name", "params"] => SyncClient::read : fn(&SyncClient, ReadName, JsonArguments) -> FacadeResult<serde_json::Value>;
    "blocks" ["finalized"] => SyncClient::blocks : fn(&SyncClient, Finality) -> FacadeResult<Blocks>;
    #[cfg(feature = "pow")]
    "mine_pow_registration" ["netuid", "hotkey_ss58", "coldkey_ss58", "workers", "max_seconds", "backend", "device_ids"] => SyncClient::mine_pow_registration : fn(&SyncClient, NetUid, AccountId, AccountId, rtao_intents::pow::PowConfig, rtao_intents::pow::CancellationToken) -> FacadeResult<rtao_intents::pow::PowProof>;
    "timestamp" ["block"] => SyncClient::timestamp : fn(&SyncClient, Option<BlockRef>) -> FacadeResult<SystemTime>;
    "block_time" [] => SyncClient::block_time : fn(&SyncClient) -> FacadeResult<f64>;
    "is_fast_blocks" [] => SyncClient::is_fast_blocks : fn(&SyncClient) -> FacadeResult<bool>;
    "block_info" ["block"] => SyncClient::block_info : fn(&SyncClient, Option<u64>) -> FacadeResult<Option<BlockInfo>>;
    "wait_for_block" ["block", "timeout"] => SyncClient::wait_for_block : fn(&SyncClient, Option<u64>, Option<Duration>) -> FacadeResult<BlockHeader>;
    "wait_for_timestamp" ["when", "timeout"] => SyncClient::wait_for_timestamp : fn(&SyncClient, SystemTime, Option<Duration>) -> FacadeResult<BlockHeader>;
    "wait_for_epoch" ["netuid", "timeout"] => SyncClient::wait_for_epoch : fn(&SyncClient, NetUid, Option<Duration>) -> FacadeResult<EpochEvent>;
    "preflight" ["intent", "wallet", "proxy_for", "proxy_type"] => SyncClient::preflight::<Transfer, KeypairSigner> : fn(&SyncClient, Transfer, Arc<KeypairSigner>, PlanOptions) -> FacadeResult<IntentPreflight>;
    "estimate_shielded_carrier_fee" ["fee_payer"] => SyncClient::estimate_shielded_carrier_fee : fn(&SyncClient, AccountId) -> FacadeResult<Balance<Tao>>;
    "plan" ["intent", "wallet", "kwargs"] => SyncClient::plan::<Transfer, KeypairSigner> : fn(&SyncClient, Transfer, Arc<KeypairSigner>, PlanOptions) -> FacadeResult<Plan>;
    "execute" ["intent", "wallet", "kwargs"] => SyncClient::execute::<Transfer, KeypairSigner> : fn(&SyncClient, Transfer, Arc<KeypairSigner>, ExecuteOptions) -> FacadeResult<ExtrinsicResult>;
    "execute_tool" ["op", "args", "wallet", "kwargs"] => SyncClient::execute_tool : fn(&SyncClient, String, serde_json::Value, DynamicKeys, ExecuteOptions) -> FacadeResult<ExtrinsicResult>;
    "submit_shielded" ["intent", "wallet", "kwargs"] => SyncClient::submit_shielded::<Transfer, KeypairSigner> : fn(&SyncClient, Transfer, Arc<KeypairSigner>, ShieldOptions) -> FacadeResult<ExtrinsicResult>;
    "submit_call" ["call", "wallet", "kwargs"] => SyncClient::submit_call::<Coldkey, KeypairSigner> : fn(&SyncClient, CallBytes, Arc<KeypairSigner>, ExecuteOptions) -> FacadeResult<ExtrinsicResult>;
    "prepare_call" ["call", "address", "kwargs"] => SyncClient::prepare_call : fn(&SyncClient, CallBytes, AccountId, PrepareOptions) -> FacadeResult<UnsignedExtrinsic>;
    "submit_signature" ["unsigned", "signature", "kwargs"] => SyncClient::submit_signature : fn(&SyncClient, UnsignedExtrinsic, MultiSignature, ExecuteOptions) -> FacadeResult<ExtrinsicResult>;
    "compose" ["call"] => SyncClient::compose : fn(&SyncClient, rtao_runtime::Call) -> FacadeResult<CallBytes>;
    "estimate_weight" ["call", "address"] => SyncClient::estimate_weight : fn(&SyncClient, CallBytes, AccountId, CryptoType) -> FacadeResult<rtao_core::ops::fee::FeeEstimate>;
    "multisig" ["signatories", "threshold"] => SyncClient::multisig : fn(&SyncClient, Vec<AccountId>, u16) -> FacadeResult<SyncMultisig>;
    "connect" [] => SyncClient::connect : fn(&SyncClient) -> FacadeResult<()>;
    "close" [] => SyncClient::close : fn(&SyncClient) -> FacadeResult<()>;
    "block" [] => SyncClient::block : fn(&SyncClient) -> FacadeResult<u64>;
    "finalized_block" [] => SyncClient::finalized_block : fn(&SyncClient) -> FacadeResult<u64>;
    "spec_version" [] => SyncClient::spec_version : fn(&SyncClient) -> FacadeResult<u32>;
    "at" ["block"] => SyncClient::at : fn(&SyncClient, BlockRef) -> FacadeResult<SyncSnapshot>;
}

// pyt: unit/test_facades.py::test_sync_facade_mirrors_async_surface[snapshot]
facade_contract! {
    snapshot_contract;
    "query" ["item", "params"] => SyncSnapshot::query : fn(&SyncSnapshot, String, String, Vec<Value>) -> FacadeResult<DynValue>;
    "query_map" ["item", "params"] => SyncSnapshot::query_map : fn(&SyncSnapshot, String, String, Vec<Value>) -> FacadeResult<MapRows>;
    "query_batch" ["item", "param_sets"] => SyncSnapshot::query_batch : fn(&SyncSnapshot, String, String, ParamSets) -> FacadeResult<Vec<DynValue>>;
    "runtime" ["method", "params"] => SyncSnapshot::runtime : fn(&SyncSnapshot, String, String, Vec<Value>) -> FacadeResult<DynValue>;
    "constant" ["item"] => SyncSnapshot::constant : fn(&SyncSnapshot, String, String) -> FacadeResult<DynValue>;
    "read" ["name", "params"] => SyncSnapshot::read : fn(&SyncSnapshot, ReadName, JsonArguments) -> FacadeResult<serde_json::Value>;
    "timestamp" [] => SyncSnapshot::timestamp : fn(&SyncSnapshot) -> FacadeResult<SystemTime>;
    "block_time" [] => SyncSnapshot::block_time : fn(&SyncSnapshot) -> FacadeResult<f64>;
    "is_fast_blocks" [] => SyncSnapshot::is_fast_blocks : fn(&SyncSnapshot) -> FacadeResult<bool>;
    "block_info" ["block"] => SyncSnapshot::block_info : fn(&SyncSnapshot, Option<u64>) -> FacadeResult<Option<BlockInfo>>;
    "at" ["block"] => SyncSnapshot::at : fn(&SyncSnapshot, Option<BlockRef>) -> FacadeResult<SyncSnapshot>;
}

// pyt: unit/test_facades.py::test_sync_client_constructs_offline
#[test]
fn finney_construction_and_close_stay_cold() -> TestResult {
    let client = SyncClient::for_network("finney")?;
    assert_eq!(client.network().label(), "finney");
    assert!(!client.is_started());
    client.close()?;
    assert!(!client.is_started());
    Ok(())
}
