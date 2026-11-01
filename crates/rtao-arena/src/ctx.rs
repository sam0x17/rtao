//! Per-thread allocation context: the active scope, the lifetime hint, the
//! declared-block marker and the conformance flag.
//!
//! On single-threaded wasm32 (no `atomics` target feature) the context is a
//! set of plain cells. With `std` elsewhere it is thread-local.
//!
//! Without `std` on any other target there are no thread-locals. The lifetime
//! hint and the declared-block marker are then process-wide atomics: another
//! thread can only change which placement or check an allocation gets. The
//! active scope is different, because allocation dereferences it, so it is
//! never process-wide: in that configuration no scope can be activated
//! ([`crate::Scope::run`] and [`crate::Scope::scoped`] are not compiled) and
//! [`in_scope`] is always false.

#![allow(unsafe_code)]

use core::marker::PhantomData;

use crate::scope::Scope;

/// Lifetime hint for large blocks.
///
/// Large blocks are carved from the top of a free block. `Short` blocks (the
/// default) take the best-fitting free block, so transients reuse the holes
/// earlier ones left; `Long` blocks take a free block of the largest size
/// class, so they pack against the large blocks above the largest free run
/// (the large side's boundary with the memory it shares with the small side)
/// and leave holes to transients.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Lifetime {
    /// Transient memory: placed at the top of the best-fitting free block.
    #[default]
    Short,
    /// Long-lived memory: placed at the top of the largest free run.
    Long,
}

impl Lifetime {
    const fn to_u8(self) -> u8 {
        match self {
            Self::Short => 0,
            Self::Long => 1,
        }
    }

    const fn from_u8(value: u8) -> Self {
        if value == 1 { Self::Long } else { Self::Short }
    }
}

#[cfg(all(target_arch = "wasm32", not(target_feature = "atomics")))]
mod imp {
    use core::cell::Cell;
    use core::ptr;

    use crate::scope::Scope;

    struct Local<T>(Cell<T>);

    // SAFETY: wasm32 without the `atomics` target feature runs exactly one
    // thread, so a `Cell` in a static is never accessed concurrently.
    unsafe impl<T> Sync for Local<T> {}

    static SCOPE: Local<*const Scope> = Local(Cell::new(ptr::null()));
    static HINT: Local<u8> = Local(Cell::new(0));
    static DECLARED: Local<bool> = Local(Cell::new(false));
    static STRICT: Local<bool> = Local(Cell::new(false));

    pub(super) fn scope() -> *const Scope {
        SCOPE.0.get()
    }
    pub(super) fn set_scope(value: *const Scope) {
        SCOPE.0.set(value);
    }
    pub(super) fn hint() -> u8 {
        HINT.0.get()
    }
    pub(super) fn set_hint(value: u8) {
        HINT.0.set(value);
    }
    pub(super) fn declared() -> bool {
        DECLARED.0.get()
    }
    pub(super) fn set_declared(value: bool) {
        DECLARED.0.set(value);
    }
    pub(super) fn strict() -> bool {
        STRICT.0.get()
    }
    #[cfg_attr(not(feature = "std"), allow(dead_code))]
    pub(super) fn set_strict(value: bool) {
        STRICT.0.set(value);
    }
}

#[cfg(all(
    any(feature = "std", test),
    not(all(target_arch = "wasm32", not(target_feature = "atomics")))
))]
mod imp {
    extern crate std;

    use core::cell::Cell;
    use core::ptr;

    use crate::scope::Scope;

    std::thread_local! {
        static SCOPE: Cell<*const Scope> = const { Cell::new(ptr::null()) };
        static HINT: Cell<u8> = const { Cell::new(0) };
        static DECLARED: Cell<bool> = const { Cell::new(false) };
        static STRICT: Cell<bool> = const { Cell::new(false) };
    }

    // The cells have no destructor, so they stay readable during thread
    // teardown; `try_with` only guards against platform corner cases.
    pub(super) fn scope() -> *const Scope {
        SCOPE.try_with(Cell::get).unwrap_or(ptr::null())
    }
    pub(super) fn set_scope(value: *const Scope) {
        let _ = SCOPE.try_with(|c| c.set(value));
    }
    pub(super) fn hint() -> u8 {
        HINT.try_with(Cell::get).unwrap_or(0)
    }
    pub(super) fn set_hint(value: u8) {
        let _ = HINT.try_with(|c| c.set(value));
    }
    pub(super) fn declared() -> bool {
        DECLARED.try_with(Cell::get).unwrap_or(false)
    }
    pub(super) fn set_declared(value: bool) {
        let _ = DECLARED.try_with(|c| c.set(value));
    }
    pub(super) fn strict() -> bool {
        STRICT.try_with(Cell::get).unwrap_or(false)
    }
    #[cfg_attr(not(feature = "std"), allow(dead_code))]
    pub(super) fn set_strict(value: bool) {
        let _ = STRICT.try_with(|c| c.set(value));
    }
}

#[cfg(all(
    not(any(feature = "std", test)),
    not(all(target_arch = "wasm32", not(target_feature = "atomics")))
))]
mod imp {
    use core::sync::atomic::{AtomicBool, AtomicU8, Ordering::Relaxed};

    use crate::scope::Scope;

    // No active scope here: a process-wide scope pointer could be read by one
    // thread while another thread's scope ends, so scopes are unavailable.
    static HINT: AtomicU8 = AtomicU8::new(0);
    static DECLARED: AtomicBool = AtomicBool::new(false);
    static STRICT: AtomicBool = AtomicBool::new(false);

    pub(super) fn scope() -> *const Scope {
        core::ptr::null()
    }
    pub(super) fn hint() -> u8 {
        HINT.load(Relaxed)
    }
    pub(super) fn set_hint(value: u8) {
        HINT.store(value, Relaxed);
    }
    pub(super) fn declared() -> bool {
        DECLARED.load(Relaxed)
    }
    pub(super) fn set_declared(value: bool) {
        DECLARED.store(value, Relaxed);
    }
    pub(super) fn strict() -> bool {
        STRICT.load(Relaxed)
    }
}

/// The scope global allocations currently go to, if any.
#[inline]
pub(crate) fn active_scope() -> *const Scope {
    imp::scope()
}

/// Whether a scope is active on this thread. Always false where scopes are
/// unavailable (see [`crate::Scope::run`]).
#[inline]
pub fn in_scope() -> bool {
    !imp::scope().is_null()
}

/// The lifetime hint large allocations on this thread currently get.
#[inline]
pub fn lifetime() -> Lifetime {
    Lifetime::from_u8(imp::hint())
}

/// Whether the current allocation is a declared block (see [`declared`]).
#[inline]
pub fn is_declared() -> bool {
    imp::declared()
}

/// Whether scopes ending with live objects must panic (conformance).
#[inline]
pub(crate) fn strict_scopes() -> bool {
    imp::strict()
}

/// Turns the scope-end conformance check on or off for this thread.
#[cfg(feature = "std")]
#[inline]
pub(crate) fn set_strict_scopes(on: bool) {
    imp::set_strict(on);
}

/// Restores the previous lifetime hint when dropped. See [`with_lifetime`].
#[must_use = "the hint is restored when the guard drops"]
#[derive(Debug)]
pub struct LifetimeGuard {
    prev: u8,
    _local: PhantomData<*const ()>,
}

impl Drop for LifetimeGuard {
    fn drop(&mut self) {
        imp::set_hint(self.prev);
    }
}

/// Sets the lifetime hint for large allocations on this thread until the
/// returned guard drops.
///
/// ```
/// use rtao_arena::{Lifetime, lifetime, with_lifetime};
///
/// assert_eq!(lifetime(), Lifetime::Short);
/// {
///     let _long = with_lifetime(Lifetime::Long);
///     assert_eq!(lifetime(), Lifetime::Long);
/// }
/// assert_eq!(lifetime(), Lifetime::Short);
/// ```
#[inline]
pub fn with_lifetime(hint: Lifetime) -> LifetimeGuard {
    let prev = imp::hint();
    imp::set_hint(hint.to_u8());
    LifetimeGuard {
        prev,
        _local: PhantomData,
    }
}

/// Clears the declared-block marker when dropped. See [`declared`].
#[must_use = "the marker is cleared when the guard drops"]
#[derive(Debug)]
pub struct DeclaredGuard {
    prev: bool,
    _local: PhantomData<*const ()>,
}

impl Drop for DeclaredGuard {
    fn drop(&mut self) {
        imp::set_declared(self.prev);
    }
}

/// Marks allocations on this thread as declared blocks until the guard drops.
///
/// A declared block is an explicit, fallible, exactly sized allocation
/// (such as [`crate::Block`] or [`crate::FixedVec`]). The conformance
/// allocator's unscoped-size check exempts declared blocks.
#[inline]
pub fn declared() -> DeclaredGuard {
    let prev = imp::declared();
    imp::set_declared(true);
    DeclaredGuard {
        prev,
        _local: PhantomData,
    }
}

/// Restores the previously active scope when dropped.
#[cfg(any(
    feature = "std",
    test,
    all(target_arch = "wasm32", not(target_feature = "atomics"))
))]
#[must_use = "the previous scope is restored when the guard drops"]
#[derive(Debug)]
pub(crate) struct ScopeGuard<'s> {
    prev: *const Scope,
    _scope: PhantomData<&'s Scope>,
}

#[cfg(any(
    feature = "std",
    test,
    all(target_arch = "wasm32", not(target_feature = "atomics"))
))]
impl Drop for ScopeGuard<'_> {
    fn drop(&mut self) {
        imp::set_scope(self.prev);
    }
}

/// Makes `scope` (or no scope, with `None`) active on this thread until the
/// guard drops.
#[cfg(any(
    feature = "std",
    test,
    all(target_arch = "wasm32", not(target_feature = "atomics"))
))]
#[inline]
pub(crate) fn enter(scope: Option<&Scope>) -> ScopeGuard<'_> {
    let prev = imp::scope();
    imp::set_scope(scope.map_or(core::ptr::null(), core::ptr::from_ref));
    ScopeGuard {
        prev,
        _scope: PhantomData,
    }
}

/// Runs `f` with no scope active, so its allocations go to the general heap
/// even inside [`Scope::run`](crate::Scope::run) or a `Scoped` poll. Where
/// scopes are unavailable no scope is ever active and this just runs `f`.
#[inline]
pub fn unscoped<R>(f: impl FnOnce() -> R) -> R {
    #[cfg(any(
        feature = "std",
        test,
        all(target_arch = "wasm32", not(target_feature = "atomics"))
    ))]
    let _guard = enter(None);
    f()
}
