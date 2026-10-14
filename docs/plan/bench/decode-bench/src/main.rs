//! Decode-strategy benchmark for the rtao design: one live finney `get_metagraph` payload
//! (and a synthesized 2,500-UID payload) decoded five ways.
use std::hint::black_box;
use std::time::{Duration, Instant};

use codec::{Compact, Decode, Encode};
use scale_decode::DecodeAsType;

// ---------- (C) parity-scale-codec mirror: owned, typed ----------
#[derive(Decode, Encode, Clone)]
struct IdentityV3 { subnet_name: Vec<u8>, github_repo: Vec<u8>, subnet_contact: Vec<u8>, subnet_url: Vec<u8>, discord: Vec<u8>, description: Vec<u8>, logo_url: Vec<u8>, additional: Vec<u8> }
#[derive(Decode, Encode, Clone)]
struct ChainIdentityV2 { name: Vec<u8>, url: Vec<u8>, github_repo: Vec<u8>, image: Vec<u8>, discord: Vec<u8>, description: Vec<u8>, additional: Vec<u8> }
#[derive(Decode, Encode, Clone)]
struct AxonInfo { block: u64, version: u32, ip: u128, port: u16, ip_type: u8, protocol: u8, placeholder1: u8, placeholder2: u8 }

#[derive(Decode, Encode, Clone)]
struct MgCodec {
    netuid: Compact<u16>, name: Vec<Compact<u8>>, symbol: Vec<Compact<u8>>, identity: Option<IdentityV3>,
    network_registered_at: Compact<u64>, owner_hotkey: [u8; 32], owner_coldkey: [u8; 32],
    block: Compact<u64>, tempo: Compact<u16>, last_step: Compact<u64>, blocks_since_last_step: Compact<u64>,
    subnet_emission: Compact<u64>, alpha_in: Compact<u64>, alpha_out: Compact<u64>, tao_in: Compact<u64>,
    alpha_out_emission: Compact<u64>, alpha_in_emission: Compact<u64>, tao_in_emission: Compact<u64>,
    pending_alpha_emission: Compact<u64>, pending_root_emission: Compact<u64>, subnet_volume: Compact<u128>,
    moving_price: i128, rho: Compact<u16>, kappa: Compact<u16>, min_allowed_weights: Compact<u16>,
    max_weights_limit: Compact<u16>, weights_version: Compact<u64>, weights_rate_limit: Compact<u64>,
    activity_cutoff: Compact<u64>, max_validators: Compact<u16>, num_uids: Compact<u16>, max_uids: Compact<u16>,
    burn: Compact<u64>, difficulty: Compact<u64>, registration_allowed: bool, pow_registration_allowed: bool,
    immunity_period: Compact<u16>, min_difficulty: Compact<u64>, max_difficulty: Compact<u64>,
    min_burn: Compact<u64>, max_burn: Compact<u64>, adjustment_alpha: Compact<u64>, adjustment_interval: Compact<u16>,
    target_regs_per_interval: Compact<u16>, max_regs_per_block: Compact<u16>, serving_rate_limit: Compact<u64>,
    commit_reveal_weights_enabled: bool, commit_reveal_period: Compact<u64>, liquid_alpha_enabled: bool,
    alpha_high: Compact<u16>, alpha_low: Compact<u16>, bonds_moving_avg: Compact<u64>,
    hotkeys: Vec<[u8; 32]>, coldkeys: Vec<[u8; 32]>, identities: Vec<Option<ChainIdentityV2>>, axons: Vec<AxonInfo>,
    active: Vec<bool>, validator_permit: Vec<bool>, pruning_score: Vec<Compact<u16>>, last_update: Vec<Compact<u64>>,
    emission: Vec<Compact<u64>>, dividends: Vec<Compact<u16>>, incentives: Vec<Compact<u16>>, consensus: Vec<Compact<u16>>,
    trust: Vec<Compact<u16>>, rank: Vec<Compact<u16>>, block_at_registration: Vec<Compact<u64>>,
    alpha_stake: Vec<Compact<u64>>, tao_stake: Vec<Compact<u64>>, total_stake: Vec<Compact<u64>>,
    tao_dividends_per_hotkey: Vec<([u8; 32], Compact<u64>)>, alpha_dividends_per_hotkey: Vec<([u8; 32], Compact<u64>)>,
    collateral_locked: Vec<Compact<u64>>, collateral_min: Vec<Compact<u64>>, collateral_earned: Vec<Compact<u64>>,
}

// ---------- (D) scale-decode DecodeAsType mirror: what subxt static codegen decodes through ----------
#[derive(DecodeAsType)]
struct AccountId(#[allow(dead_code)] [u8; 32]);
#[derive(DecodeAsType)]
#[allow(dead_code)]
struct IdentityV3D { subnet_name: Vec<u8>, github_repo: Vec<u8>, subnet_contact: Vec<u8>, subnet_url: Vec<u8>, discord: Vec<u8>, description: Vec<u8>, logo_url: Vec<u8>, additional: Vec<u8> }
#[derive(DecodeAsType)]
#[allow(dead_code)]
struct ChainIdentityV2D { name: Vec<u8>, url: Vec<u8>, github_repo: Vec<u8>, image: Vec<u8>, discord: Vec<u8>, description: Vec<u8>, additional: Vec<u8> }
#[derive(DecodeAsType)]
#[allow(dead_code)]
struct AxonInfoD { block: u64, version: u32, ip: u128, port: u16, ip_type: u8, protocol: u8, placeholder1: u8, placeholder2: u8 }
#[derive(DecodeAsType)]
#[allow(dead_code)]
struct Fixed { bits: i128 }
#[derive(DecodeAsType)]
#[allow(dead_code)]
struct MgDat {
    netuid: u16, name: Vec<u8>, symbol: Vec<u8>, identity: Option<IdentityV3D>, network_registered_at: u64,
    owner_hotkey: AccountId, owner_coldkey: AccountId, block: u64, tempo: u16, last_step: u64, blocks_since_last_step: u64,
    subnet_emission: u64, alpha_in: u64, alpha_out: u64, tao_in: u64, alpha_out_emission: u64, alpha_in_emission: u64,
    tao_in_emission: u64, pending_alpha_emission: u64, pending_root_emission: u64, subnet_volume: u128, moving_price: Fixed,
    rho: u16, kappa: u16, min_allowed_weights: u16, max_weights_limit: u16, weights_version: u64, weights_rate_limit: u64,
    activity_cutoff: u64, max_validators: u16, num_uids: u16, max_uids: u16, burn: u64, difficulty: u64,
    registration_allowed: bool, pow_registration_allowed: bool, immunity_period: u16, min_difficulty: u64, max_difficulty: u64,
    min_burn: u64, max_burn: u64, adjustment_alpha: u64, adjustment_interval: u16, target_regs_per_interval: u16,
    max_regs_per_block: u16, serving_rate_limit: u64, commit_reveal_weights_enabled: bool, commit_reveal_period: u64,
    liquid_alpha_enabled: bool, alpha_high: u16, alpha_low: u16, bonds_moving_avg: u64,
    hotkeys: Vec<AccountId>, coldkeys: Vec<AccountId>, identities: Vec<Option<ChainIdentityV2D>>, axons: Vec<AxonInfoD>,
    active: Vec<bool>, validator_permit: Vec<bool>, pruning_score: Vec<u16>, last_update: Vec<u64>, emission: Vec<u64>,
    dividends: Vec<u16>, incentives: Vec<u16>, consensus: Vec<u16>, trust: Vec<u16>, rank: Vec<u16>,
    block_at_registration: Vec<u64>, alpha_stake: Vec<u64>, tao_stake: Vec<u64>, total_stake: Vec<u64>,
    tao_dividends_per_hotkey: Vec<(AccountId, u64)>, alpha_dividends_per_hotkey: Vec<(AccountId, u64)>,
    collateral_locked: Vec<u64>, collateral_min: Vec<u64>, collateral_earned: Vec<u64>,
}


#[derive(DecodeAsType)]
#[allow(dead_code)]
#[decode_as_type(trait_bounds = "")]
struct MgDatHot {
    netuid: u16, block: u64, tempo: u16, num_uids: u16,
    hotkeys: Vec<[u8; 32]>, coldkeys: Vec<[u8; 32]>, axons: Vec<AxonInfoD>,
    active: Vec<bool>, validator_permit: Vec<bool>, last_update: Vec<u64>, emission: Vec<u64>,
    dividends: Vec<u16>, incentives: Vec<u16>, consensus: Vec<u16>, block_at_registration: Vec<u64>,
    alpha_stake: Vec<u64>, tao_stake: Vec<u64>, total_stake: Vec<u64>,
}

// ---------- (E) borrowed columnar view: fixed-width columns borrowed, compact columns decoded once ----------
struct Rd<'a> { b: &'a [u8], o: usize }
impl<'a> Rd<'a> {
    fn take(&mut self, n: usize) -> &'a [u8] { let s = &self.b[self.o..self.o + n]; self.o += n; s }
    fn u8(&mut self) -> u8 { self.take(1)[0] }
    fn compact(&mut self) -> u128 {
        let b0 = self.b[self.o];
        match b0 & 3 {
            0 => { self.o += 1; (b0 >> 2) as u128 }
            1 => { let v = u16::from_le_bytes([self.b[self.o], self.b[self.o + 1]]); self.o += 2; (v >> 2) as u128 }
            2 => { let v = u32::from_le_bytes(self.b[self.o..self.o + 4].try_into().unwrap()); self.o += 4; (v >> 2) as u128 }
            _ => { let n = (b0 >> 2) as usize + 4; self.o += 1; let mut buf = [0u8; 16]; buf[..n].copy_from_slice(self.take(n)); u128::from_le_bytes(buf) }
        }
    }
    fn skip_bytes(&mut self) { let n = self.compact() as usize; self.o += n; }
    fn compact_vec_u64(&mut self) -> Vec<u64> { let n = self.compact() as usize; let mut v = Vec::with_capacity(n); for _ in 0..n { v.push(self.compact() as u64); } v }
    fn compact_vec_u16(&mut self) -> Vec<u16> { let n = self.compact() as usize; let mut v = Vec::with_capacity(n); for _ in 0..n { v.push(self.compact() as u16); } v }
    fn fixed_vec(&mut self, width: usize) -> &'a [u8] { let n = self.compact() as usize; self.take(n * width) }
}

#[allow(dead_code)]
struct MgView<'a> {
    netuid: u16, block: u64, tempo: u16, num_uids: u16,
    hotkeys: &'a [u8], coldkeys: &'a [u8], axons: &'a [u8], active: &'a [u8], validator_permit: &'a [u8],
    last_update: Vec<u64>, emission: Vec<u64>, dividends: Vec<u16>, incentives: Vec<u16>, consensus: Vec<u16>,
    block_at_registration: Vec<u64>, alpha_stake: Vec<u64>, tao_stake: Vec<u64>, total_stake: Vec<u64>,
    collateral_locked: Vec<u64>, collateral_min: Vec<u64>, collateral_earned: Vec<u64>,
}

fn view(bytes: &[u8]) -> MgView<'_> {
    let mut r = Rd { b: bytes, o: 0 };
    assert_eq!(r.u8(), 1, "Option::Some");
    let netuid = r.compact() as u16;
    for _ in 0..2 { let n = r.compact() as usize; for _ in 0..n { r.compact(); } } // name, symbol: Vec<Compact<u8>> (bytes >= 64 take 2 bytes)
    if r.u8() == 1 { for _ in 0..8 { r.skip_bytes(); } }
    r.compact(); r.take(64);
    let block = r.compact() as u64; let tempo = r.compact() as u16; r.compact(); r.compact();
    for _ in 0..10 { r.compact(); } // emission terms incl. subnet_volume
    r.take(16); // moving_price
    for _ in 0..8 { r.compact(); } // rho..max_validators
    let num_uids = r.compact() as u16; r.compact(); r.compact(); r.compact(); // num_uids, max_uids, burn, difficulty
    r.take(2); // two bools
    for _ in 0..10 { r.compact(); } // immunity..serving_rate_limit
    r.take(1); r.compact(); r.take(1); r.compact(); r.compact(); r.compact();
    let hotkeys = r.fixed_vec(32); let coldkeys = r.fixed_vec(32);
    let n = r.compact() as usize; for _ in 0..n { if r.u8() == 1 { for _ in 0..7 { r.skip_bytes(); } } }
    let axons = r.fixed_vec(34); let active = r.fixed_vec(1); let validator_permit = r.fixed_vec(1);
    let _pruning = r.compact_vec_u16();
    let last_update = r.compact_vec_u64(); let emission = r.compact_vec_u64();
    let dividends = r.compact_vec_u16(); let incentives = r.compact_vec_u16(); let consensus = r.compact_vec_u16();
    let _trust = r.compact_vec_u16(); let _rank = r.compact_vec_u16();
    let block_at_registration = r.compact_vec_u64(); let alpha_stake = r.compact_vec_u64(); let tao_stake = r.compact_vec_u64(); let total_stake = r.compact_vec_u64();
    for _ in 0..2 { let n = r.compact() as usize; for _ in 0..n { r.take(32); r.compact(); } }
    let collateral_locked = r.compact_vec_u64(); let collateral_min = r.compact_vec_u64(); let collateral_earned = r.compact_vec_u64();
    assert_eq!(r.o, bytes.len(), "view consumed every byte");
    MgView { netuid, block, tempo, num_uids, hotkeys, coldkeys, axons, active, validator_permit, last_update, emission, dividends, incentives, consensus, block_at_registration, alpha_stake, tao_stake, total_stake, collateral_locked, collateral_min, collateral_earned }
}

fn bench(name: &str, bytes: usize, iters: usize, mut f: impl FnMut() -> usize) {
    for _ in 0..(iters / 10 + 1) { black_box(f()); }
    let mut s: Vec<Duration> = (0..iters).map(|_| { let t = Instant::now(); black_box(f()); t.elapsed() }).collect();
    s.sort();
    let med = s[s.len() / 2];
    let mbps = bytes as f64 / med.as_secs_f64() / 1e6;
    println!("  {name:<44} median {:>10.1} us   p10 {:>10.1} us   {:>8.1} MB/s", med.as_secs_f64() * 1e6, s[s.len() / 10].as_secs_f64() * 1e6, mbps);
}

fn strip_option_vec(hexs: &str) -> Vec<u8> {
    let raw = hex::decode(hexs.trim().trim_start_matches("0x")).unwrap();
    let mut input = &raw[..];
    let opt: Option<Vec<u8>> = Decode::decode(&mut input).unwrap();
    opt.unwrap()
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "data".into());
    let md = strip_option_vec(&std::fs::read_to_string(format!("{dir}/v15.optionhex")).unwrap());
    println!("V15 metadata: {} bytes", md.len());
    let t = Instant::now();
    let mut n = 0;
    for _ in 0..20 { let rt = bittensor_core::runtime::Runtime::parse(&md, 477, 1, 42).unwrap(); n += rt.pallets.len(); }
    println!("bittensor-core Runtime::parse (V15, spec 477): {:.2} ms avg ({n})", t.elapsed().as_secs_f64() * 1e3 / 20.0);
    let rt = bittensor_core::runtime::Runtime::parse(&md, 477, 1, 42).unwrap();
    println!("V15 extrinsic version {} ; extensions: {:?}", rt.extrinsic.version, rt.extrinsic.signed_extensions.iter().map(|e| e.identifier.as_str()).collect::<Vec<_>>());
    println!("pallets {} ; apis {} ; is_v15 {}", rt.pallets.len(), rt.apis.len(), rt.is_v15);
    if std::env::var("ONLY_INFO").is_ok() { return; }
    let api = rt.apis.iter().find(|a| a.name == "SubnetInfoRuntimeApi").unwrap();
    let out_id = api.methods.iter().find(|m| m.name == "get_metagraph").unwrap().output;
    let prefixed = frame_metadata::RuntimeMetadataPrefixed::decode(&mut &md[..]).unwrap();
    let registry = match prefixed.1 { frame_metadata::RuntimeMetadata::V15(m) => m.types, _ => panic!() };

    let mut payloads: Vec<(String, Vec<u8>)> = Vec::new();
    for netuid in [1u16, 3, 64] {
        let h = std::fs::read_to_string(format!("{dir}/mg_{netuid}.hex")).unwrap();
        payloads.push((format!("finney netuid {netuid}"), hex::decode(h.trim().trim_start_matches("0x")).unwrap()));
    }
    // Synthesize a 2,500-UID metagraph from netuid 1's real columns (Null-consensus budget, NULL_UID_BUDGET).
    let base: Option<MgCodec> = Decode::decode(&mut &payloads[0].1[..]).unwrap();
    let mut big = base.clone().unwrap();
    let n0 = big.hotkeys.len();
    macro_rules! grow { ($($f:ident),*) => { $( let src = big.$f.clone(); big.$f = (0..2500).map(|i| src[i % n0].clone()).collect(); )* } }
    grow!(hotkeys, coldkeys, identities, axons, active, validator_permit, last_update, emission, dividends, incentives, consensus, block_at_registration, alpha_stake, tao_stake, total_stake, collateral_locked, collateral_min, collateral_earned);
    if !big.pruning_score.is_empty() { let s = big.pruning_score.clone(); big.pruning_score = (0..2500).map(|i| s[i % s.len()]).collect(); }
    let ad = big.alpha_dividends_per_hotkey.clone(); if !ad.is_empty() { big.alpha_dividends_per_hotkey = (0..2500).map(|i| ad[i % ad.len()]).collect(); }
    big.num_uids = Compact(2500);
    payloads.push(("synthetic 2,500 UIDs (from netuid 1)".into(), Some(big).encode()));

    for (label, bytes) in &payloads {
        let uids = { let v: Option<MgCodec> = Decode::decode(&mut &bytes[..]).unwrap(); v.unwrap().hotkeys.len() };
        println!("\n{label}: {} bytes SCALE, {uids} UIDs", bytes.len());
        let iters = if bytes.len() > 200_000 { 60 } else { 300 };
        bench("A bittensor-core Runtime::decode_id -> Value", bytes.len(), iters, || {
            let mut c = bittensor_core::codec::decode::Cursor::new(bytes);
            match rt.decode_id(out_id, &mut c).unwrap() { bittensor_core::codec::Value::Dict(d) => d.len(), _ => 0 }
        });
        bench("B scale-value decode_as_type -> Value", bytes.len(), iters, || {
            let v = scale_value::scale::decode_as_type(&mut &bytes[..], out_id, &registry).unwrap();
            std::mem::size_of_val(&v)
        });
        bench("C parity-scale-codec derive (owned, typed)", bytes.len(), iters, || {
            let v: Option<MgCodec> = Decode::decode(&mut &bytes[..]).unwrap();
            v.map_or(0, |m| m.hotkeys.len())
        });
        bench("D scale-decode DecodeAsType (subxt static)", bytes.len(), iters, || {
            let v = <Option<MgDat> as DecodeAsType>::decode_as_type(&mut &bytes[..], out_id, &registry).unwrap();
            v.map_or(0, |m| m.hotkeys.len())
        });
        bench("D2 DecodeAsType, [u8;32] keys, hot fields only", bytes.len(), iters, || {
            let v = <Option<MgDatHot> as DecodeAsType>::decode_as_type(&mut &bytes[..], out_id, &registry).unwrap();
            v.map_or(0, |m| m.hotkeys.len())
        });
        bench("E borrowed columnar view (rtao proposal)", bytes.len(), iters, || view(bytes).hotkeys.len());
        let ss58 = { let t = Instant::now(); let mut k = 0usize; let m: Option<MgCodec> = Decode::decode(&mut &bytes[..]).unwrap(); let m = m.unwrap();
            for h in m.hotkeys.iter().chain(m.coldkeys.iter()) { k += bittensor_core::keys::ss58_from_public(*h, 42).len(); } (t.elapsed(), k) };
        println!("  (ss58-encode every hotkey+coldkey once: {:.1} us)", ss58.0.as_secs_f64() * 1e6);
    }
}
