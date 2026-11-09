// Derived from RaoFoundation/subtensor sdk/python/bittensor/hashed.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
//! Protected (post-quantum) account records and the shared registration reserve.
//!
//! These are the reads Python's `hashed` module makes before composing a call
//! that establishes a protected account, such as a pure proxy spawned by a
//! quantum-resistant payer. They live in `rtao_tx::hashed`, beside the
//! recipient registration guards that use them (D-148).

pub(crate) use rtao_tx::hashed::{
    DERIVED_REGISTRATION_DEPOSIT, is_pq_required, quote_registration_deposit,
};
