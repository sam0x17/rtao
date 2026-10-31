//! The conformance allocator as this test binary's global allocator.

#![cfg(feature = "std")]
#![allow(unsafe_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::process::Command;

use rtao_arena::conformance::{OnViolation, Shape, Strict, ViolationKind};
use rtao_arena::{Block, Lifetime, Scope};

const MIB: usize = 1 << 20;
const ARENA: usize = 48 * MIB;
const CHILD: &str = "RTAO_ARENA_ABORT_CHILD";

#[global_allocator]
static STRICT: Strict = Strict::new(
    Shape::new(ARENA, 4 * MIB, 8 * MIB)
        .unscoped_threshold(256 << 10)
        .on_violation(OnViolation::Record),
);

#[test]
fn strict_checks_the_live_path() {
    if std::env::var_os(CHILD).is_some() {
        return;
    }
    // Ordinary allocations come from the arena.
    let boxed = Box::new([7u8; 100]);
    assert!(STRICT.contains(boxed.as_ptr()));
    let large = vec![1u8; MIB];
    assert!(STRICT.contains(large.as_ptr()));
    drop(large);
    let base = STRICT.violations();

    // Unscoped large allocations on the live path are violations ...
    STRICT.live_path(true);
    let unscoped = Vec::<u8>::with_capacity(MIB);
    STRICT.live_path(false);
    assert_eq!(STRICT.violations(), base + 1);
    assert_eq!(
        STRICT.last_violation().unwrap().kind,
        ViolationKind::UnscopedLarge
    );
    assert_eq!(STRICT.last_violation().unwrap().size, MIB);
    drop(unscoped);

    // ... unless they are declared blocks or inside a scope.
    STRICT.live_path(true);
    let block = Block::try_new_zeroed(MIB, Lifetime::Long).unwrap();
    let scope = Scope::new("strict-scope", 4 * MIB, MIB);
    let scoped = scope.run(|| Vec::<u8>::with_capacity(512 << 10));
    assert_eq!(scope.stats().objects, 1);
    drop(scoped);
    drop(scope);
    STRICT.live_path(false);
    assert_eq!(STRICT.violations(), base + 1);
    drop(block);
    assert_eq!(
        STRICT.scope_summary("strict-scope").unwrap().peak,
        512 << 10
    );

    // Exhaustion on the live path is a violation; the allocation fails.
    STRICT.live_path(true);
    let declared = rtao_arena::declared();
    let mut huge = Vec::<u8>::new();
    assert!(huge.try_reserve_exact(ARENA).is_err());
    drop(declared);
    STRICT.live_path(false);
    assert_eq!(STRICT.violations(), base + 2);
    assert_eq!(
        STRICT.last_violation().unwrap().kind,
        ViolationKind::Exhausted
    );

    // Off the live path it is served elsewhere and counted as growth.
    let growth = STRICT.growth_requests();
    let mut outside = Vec::<u8>::new();
    outside.try_reserve_exact(ARENA).unwrap();
    assert!(!STRICT.contains(outside.as_ptr()));
    assert!(STRICT.growth_requests() > growth);
    drop(outside);

    // A scope that ends with live objects panics on the live path.
    STRICT.live_path(true);
    let scope = Scope::new("outlives", MIB, 64 << 10);
    let kept = scope.run(|| Box::new([3u8; 64]));
    let ended = catch_unwind(AssertUnwindSafe(move || drop(scope)));
    STRICT.live_path(false);
    assert!(ended.is_err());
    assert_eq!(STRICT.stats().parked_chunks, 1);
    drop(kept);
    assert_eq!(STRICT.stats().parked_chunks, 0);

    // Peaks per side stay within the shape.
    let stats = STRICT.stats();
    assert!(stats.small.peak > 0 && stats.large.peak > MIB);
    assert!(stats.small.capacity <= stats.region - 8 * MIB);
    assert!(STRICT.high_water() <= ARENA);
}

#[test]
fn abort_mode_aborts_with_the_size() {
    if std::env::var_os(CHILD).is_some() {
        STRICT.set_on_violation(OnViolation::Abort);
        STRICT.live_path(true);
        let _declared = rtao_arena::declared();
        let mut huge = Vec::<u8>::new();
        let _ = huge.try_reserve_exact(ARENA);
        unreachable!("the violation aborts");
    }
    let exe = std::env::current_exe().unwrap();
    let output = Command::new(exe)
        .args([
            "--exact",
            "abort_mode_aborts_with_the_size",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD, "1")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!(
            "rtao-arena conformance: arena exhausted of {ARENA} bytes"
        )),
        "{stderr}"
    );
}

const CASE: &str = "RTAO_ARENA_STRICT_CASE";

/// Runs the test `name` alone in a child process, so the violations it
/// records stay out of this process's counters.
fn in_child(name: &str) -> bool {
    if std::env::var_os(CASE).is_some() {
        return true;
    }
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--nocapture", "--test-threads=1"])
        .env(CASE, "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{name}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    false
}

#[test]
fn realloc_growth_past_the_threshold_is_checked() {
    if !in_child("realloc_growth_past_the_threshold_is_checked") {
        return;
    }
    let base = STRICT.violations();
    STRICT.live_path(true);
    // Growth by reserve ...
    let mut reserved: Vec<u8> = Vec::with_capacity(1024);
    reserved.reserve_exact(4 * MIB);
    // ... and by pushing, as most buffers grow.
    let mut pushed: Vec<u8> = Vec::new();
    for i in 0..(1 << 20) {
        pushed.push(i as u8);
    }
    STRICT.live_path(false);
    // One for the reserve, two for the pushes (to 512 KiB, then 1 MiB).
    assert_eq!(STRICT.violations(), base + 3);
    let last = STRICT.last_violation().unwrap();
    assert_eq!((last.kind, last.size), (ViolationKind::UnscopedLarge, MIB));
    drop((reserved, pushed));

    // Growth inside a scope or a declared block, and growth below the
    // threshold, is fine.
    STRICT.live_path(true);
    let scope = Scope::new("grow", 8 * MIB, MIB);
    let scoped = scope.run(|| {
        let mut v: Vec<u8> = Vec::new();
        v.reserve_exact(MIB);
        v
    });
    drop(scoped);
    drop(scope);
    let declared = rtao_arena::declared();
    let mut v: Vec<u8> = Vec::with_capacity(1024);
    v.reserve_exact(MIB);
    drop(declared);
    let mut small: Vec<u8> = Vec::with_capacity(1024);
    small.reserve_exact(128 << 10);
    STRICT.live_path(false);
    drop((v, small));
    assert_eq!(STRICT.violations(), base + 3);
}

#[test]
fn scope_cap_refusals_are_limits_not_exhaustion() {
    if !in_child("scope_cap_refusals_are_limits_not_exhaustion") {
        return;
    }
    let base = STRICT.violations();
    let refusals = STRICT.stats().scope_cap_refusals;
    STRICT.live_path(true);
    let scope = Scope::new("capped", 128 << 10, 64 << 10);
    // A fresh allocation over the cap ...
    let fresh = scope.run(|| Vec::<u8>::new().try_reserve_exact(MIB).is_err());
    // ... and growth of a scope object past it both fail fallibly.
    let mut grown = scope.run(|| Vec::<u8>::with_capacity(1000));
    let grew = scope.run(|| grown.try_reserve_exact(MIB).is_ok());
    assert!(STRICT.contains(grown.as_ptr()));
    drop(grown);
    drop(scope);
    STRICT.live_path(false);
    assert!(fresh && !grew);
    assert_eq!(STRICT.violations(), base, "{:?}", STRICT.last_violation());
    assert_eq!(STRICT.stats().scope_cap_refusals, refusals + 2);
}

#[test]
fn scope_caps_hold_off_the_live_path() {
    // Off the live path a cap refusal is still the caller's limit: it is not
    // served by the system allocator outside the arena.
    let scope = Scope::new("capped-off", 128 << 10, 64 << 10);
    let mut fresh = Vec::<u8>::new();
    assert!(scope.run(|| fresh.try_reserve_exact(MIB)).is_err());
    let mut grown = scope.run(|| Vec::<u8>::with_capacity(16));
    assert!(scope.run(|| grown.try_reserve_exact(MIB)).is_err());
    assert!(STRICT.contains(grown.as_ptr()));
    drop(grown);
    let stats = scope.finish();
    assert_eq!((stats.cap_refusals, stats.parked), (2, 0));
}
