// Derived from RaoFoundation/subtensor sdk/python/bittensor/hashed.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
//! Hashed-account reads and registration guards for write paths (D-148).
//!
//! A payment or ownership change naming a hashed or post-quantum account must
//! not reach a bare, possibly unregistered account. Python's
//! `with_recipient_registration` puts the recipient's registration (first
//! use) or a check that it is still registered with the same descriptor in one
//! atomic `Utility.batch_all` with the operation; operations that cannot be
//! batched (an announced coldkey swap, PoW registration, announced proxy
//! execution) instead require the registration in finalized state, which is
//! permanent. Planning and submission compose the same call, so the previewed
//! call and fee are the submitted ones, and no private key is read here.
//!
//! Every read queries live metadata and the pinned state on each call: sudo
//! can change the switch and the reserve without changing the runtime.

use std::{collections::BTreeMap, future::Future};

use rtao_client::{View, ViewExt};
use rtao_core::codec::Value as CoreValue;
use rtao_primitives::{
    AccountId, Balance, Tao, TaoBalance,
    receiving::{
        Recipient, descriptor_account, descriptor_bytes, descriptor_storage, is_receiving_address,
        is_stateless_pq_descriptor, parse_recipient, raw_bytes, registration_descriptor_value,
    },
};
use rtao_runtime::{
    CallBytes, ConstantDescriptor, RuntimeHandle,
    calls::{hashed_accounts as calls, utility},
    constants::hashed_accounts as consts,
    storage::{admin_utils, hashed_accounts as st},
};
use rtao_signer::KeyView;
use serde_json::{Map, Value};

use crate::{DynIntent, IntentError, OriginView};

/// Operations that may register their recipient in the same batch, by the
/// field naming it (Python's `_REGISTRATION_TARGETS`).
const REGISTRATION_TARGETS: &[(&str, &str)] = &[
    ("transfer", "dest_ss58"),
    ("transfer_all", "dest_ss58"),
    ("announce_coldkey_swap", "new_coldkey_ss58"),
    ("swap_coldkey_announced", "new_coldkey_ss58"),
    ("swap_hotkey", "new_hotkey_ss58"),
    ("burned_register", "hotkey_ss58"),
    ("pow_register", "hotkey_ss58"),
    ("root_register", "hotkey_ss58"),
    ("register_subnet", "hotkey_ss58"),
];

/// Operations whose target is checked for its descriptor (Python's
/// `_DESCRIPTOR_TARGETS`): association checks ownership but never creates an
/// account under a possibly incompatible owner.
fn descriptor_target(op: &str) -> Option<&'static str> {
    REGISTRATION_TARGETS
        .iter()
        .find(|(name, _)| *name == op)
        .map(|(_, field)| *field)
        .or((op == "associate_hotkey").then_some("hotkey_ss58"))
}

fn is_registration_target(op: &str) -> bool {
    REGISTRATION_TARGETS.iter().any(|(name, _)| *name == op)
}

fn has_constant(runtime: &RuntimeHandle, pallet: &str, name: &str) -> bool {
    runtime.core().constant(pallet, name).is_some()
}

fn validation(message: impl Into<String>) -> IntentError {
    IntentError::validation(message)
}

/// Which permanent registry protects an account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtectedKind {
    /// A version-1 hashed account (`HashedAccounts.Accounts`).
    V1,
    /// A direct post-quantum account (`HashedAccounts.MlDsaAccounts`).
    MlDsa,
    /// A derived authority policy (`HashedAccounts.DerivedAuthorities`).
    Derived,
}

/// Python's `protected_account_record`: direct and derived protection,
/// without querying absent post-quantum metadata.
///
/// Derived records are authority policies, never direct signing descriptors.
#[doc(alias = "bittensor.hashed.protected_account_record")]
pub async fn protected_account_kind<V: View + ?Sized>(
    view: &V,
    runtime: &RuntimeHandle,
    account: AccountId,
) -> Result<Option<ProtectedKind>, IntentError> {
    if runtime
        .storage_entry("HashedAccounts", "Accounts")
        .is_none()
    {
        // Runtimes before the hashed-account pallet protect nothing.
        return Ok(None);
    }
    if view.storage(&st::ACCOUNTS, (account,)).await?.is_some() {
        return Ok(Some(ProtectedKind::V1));
    }
    if has_constant(runtime, "HashedAccounts", "MlDsaRegistrationDeposit") {
        if view
            .storage(&st::ML_DSA_ACCOUNTS, (account,))
            .await?
            .is_some()
        {
            return Ok(Some(ProtectedKind::MlDsa));
        }
        if view
            .storage(&st::DERIVED_AUTHORITIES, (account,))
            .await?
            .is_some()
        {
            return Ok(Some(ProtectedKind::Derived));
        }
    }
    Ok(None)
}

/// Python's `is_pq_required`: the permanent direct or derived
/// quantum-resistant authority floor.
#[doc(alias = "bittensor.hashed.is_pq_required")]
pub async fn is_pq_required<V: View + ?Sized>(
    view: &V,
    runtime: &RuntimeHandle,
    account: AccountId,
) -> Result<bool, IntentError> {
    if !has_constant(runtime, "HashedAccounts", "MlDsaRegistrationDeposit") {
        return Ok(false);
    }
    Ok(matches!(
        protected_account_kind(view, runtime, account).await?,
        Some(ProtectedKind::MlDsa | ProtectedKind::Derived)
    ))
}

/// The live shared registration reserve and whether a call may bound it.
#[doc(alias = "bittensor.hashed.HashedRegistrationDepositQuote")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DepositQuote {
    /// Reserve in rao, possibly zero.
    pub amount_rao: u64,
    /// The runtime stores a sudo-configurable reserve (and accepts its guard,
    /// `HashedAccounts.check_registration_deposit`).
    pub configurable: bool,
}

/// Python's `quote_hashed_registration_deposit`: the live shared reserve at
/// the pinned state, including zero.
///
/// Only runtimes whose metadata lacks the storage entry use the older
/// per-kind constant; a failed read never turns into an approved cost.
#[doc(alias = "bittensor.hashed.quote_hashed_registration_deposit")]
pub async fn quote_registration_deposit<V: View + ?Sized>(
    view: &V,
    runtime: &RuntimeHandle,
    legacy: &ConstantDescriptor<TaoBalance>,
) -> Result<DepositQuote, IntentError> {
    let (amount, configurable) = if runtime
        .storage_entry("HashedAccounts", "HashedAccountRegistrationDeposit")
        .is_some()
    {
        (
            view.storage(&st::HASHED_ACCOUNT_REGISTRATION_DEPOSIT, ())
                .await?,
            true,
        )
    } else if has_constant(runtime, legacy.surface().pallet, legacy.surface().name) {
        (view.constant(legacy).await?, false)
    } else {
        return Err(validation(
            "could not quote hashed account registration reserve",
        ));
    };
    Ok(DepositQuote {
        amount_rao: amount.0,
        configurable,
    })
}

/// The derived-account (pure proxy) reserve constant used by older runtimes.
pub const DERIVED_REGISTRATION_DEPOSIT: &ConstantDescriptor<TaoBalance> =
    &consts::DERIVED_REGISTRATION_DEPOSIT;

/// Python's `hashed_accounts_enabled`: the live switch, or `None` when this
/// runtime has no hashed-account support (`HashedAccounts.RegistrationDeposit`
/// absent). Never cached: sudo can change it without changing metadata.
#[doc(alias = "bittensor.hashed.hashed_accounts_enabled")]
pub async fn hashed_accounts_enabled<V: View + ?Sized>(
    view: &V,
    runtime: &RuntimeHandle,
) -> Result<Option<bool>, IntentError> {
    if !has_constant(runtime, "HashedAccounts", "RegistrationDeposit") {
        return Ok(None);
    }
    if runtime
        .storage_entry("AdminUtils", "HashedAccountsEnabled")
        .is_none()
    {
        return Ok(None);
    }
    Ok(Some(
        view.storage(&admin_utils::HASHED_ACCOUNTS_ENABLED, ())
            .await?,
    ))
}

/// Python's `descriptor_registration_enabled`: whether a new account with
/// this descriptor can be registered now; `None` when the runtime does not
/// support its scheme.
#[doc(alias = "bittensor.hashed.descriptor_registration_enabled")]
pub async fn descriptor_registration_enabled<V: View + ?Sized>(
    view: &V,
    runtime: &RuntimeHandle,
    descriptor: &[u8],
) -> Result<Option<bool>, IntentError> {
    if descriptor_storage(descriptor)? == "Accounts" {
        return hashed_accounts_enabled(view, runtime).await;
    }
    let profile = descriptor[1];
    // The core checks the registration variant and the advertised profiles
    // (Python's `supports_fixed_profile`); a rotating ML-DSA descriptor also
    // needs the ML-DSA reserve, as Python.
    if !runtime.core().supports_mldsa_profile(profile)
        || (profile == 4 && !has_constant(runtime, "HashedAccounts", "MlDsaRegistrationDeposit"))
    {
        return Ok(None);
    }
    hashed_accounts_enabled(view, runtime).await
}

/// Python's `_require_receiving_support`.
pub async fn require_receiving_support<V: View + ?Sized>(
    view: &V,
    runtime: &RuntimeHandle,
    descriptor: &[u8],
) -> Result<(), IntentError> {
    if descriptor_registration_enabled(view, runtime, descriptor).await? != Some(true) {
        return Err(validation(
            "hashed receiving addresses are not supported on this chain",
        ));
    }
    Ok(())
}

/// Python's `is_address_param`: a field carrying an address or a list of them.
pub fn is_address_param(name: &str) -> bool {
    name.ends_with("_ss58") || name.ends_with("_ss58s")
}

/// The receiving addresses among an intent's address fields (Python's
/// `prepare_recipient_intent` recipients), keyed by field name (`name` for a
/// scalar, `name[index]` for a list, `children[index][1]` for `set_children`),
/// in the catalog's field order.
///
/// Each one is decoded by the core's strict codec; SS58 fields are not
/// returned. Decoding never reads the chain.
#[doc(alias = "bittensor.hashed.prepare_recipient_intent")]
pub fn receiving_recipients(
    intent: &dyn DynIntent,
) -> Result<Vec<(String, Recipient)>, IntentError> {
    let spec = intent.spec();
    let args = intent.to_json_dyn();
    let Some(args) = args.as_object() else {
        return Ok(Vec::new());
    };
    let mut found = Vec::new();
    let mut add = |name: String, value: &Value| -> Result<(), IntentError> {
        if let Some(text) = value.as_str()
            && is_receiving_address(text)
        {
            found.push((name, parse_recipient(text)?));
        }
        Ok(())
    };
    let mut names: Vec<&str> = spec.fields.iter().map(|field| field.name).collect();
    for name in args.keys() {
        if !names.contains(&name.as_str()) {
            names.push(name);
        }
    }
    for name in names {
        let Some(value) = args.get(name) else {
            continue;
        };
        if spec.name == "set_children" && name == "children" {
            for (index, pair) in value.as_array().into_iter().flatten().enumerate() {
                if let Some(address) = pair.as_array().and_then(|pair| pair.get(1)) {
                    add(format!("children[{index}][1]"), address)?;
                }
            }
            continue;
        }
        if !is_address_param(name) {
            continue;
        }
        match value {
            Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    add(format!("{name}[{index}]"), item)?;
                }
            }
            value => add(name.to_owned(), value)?,
        }
    }
    Ok(found)
}

/// Whether imported (pinned) call bytes must be rebuilt and compared because
/// the intent carries recipient setup inputs (Python's
/// `has_receiving_setup_inputs`): a receiving address, an explicit
/// `hashed_descriptor` or a local hashed recipient, at any nesting depth.
///
/// This is conservative: it also looks inside nested JSON (multisig, proxy
/// announcement, crowdloan and batch children) without decoding them, so it
/// may rebuild where Python would not; a rebuild must reproduce the pinned
/// bytes exactly.
#[doc(alias = "bittensor.hashed.has_receiving_setup_inputs")]
pub fn has_receiving_setup_inputs(
    intent: &dyn DynIntent,
    keys: &dyn KeyView,
) -> Result<bool, IntentError> {
    if !receiving_recipients(intent)?.is_empty() {
        return Ok(true);
    }
    fn walk(value: &Value, keys: &dyn KeyView, depth: usize) -> Result<bool, IntentError> {
        if depth > 16 {
            return Err(validation(
                "intent nesting is too deep to verify recipient setup",
            ));
        }
        Ok(match value {
            Value::String(text) => {
                is_receiving_address(text)
                    || text
                        .parse::<AccountId>()
                        .is_ok_and(|account| keys.local_descriptor(&account).is_some())
            }
            Value::Array(items) => {
                for item in items {
                    if walk(item, keys, depth + 1)? {
                        return Ok(true);
                    }
                }
                false
            }
            Value::Object(fields) => {
                if fields
                    .get("hashed_descriptor")
                    .is_some_and(|value| !value.is_null())
                {
                    return Ok(true);
                }
                for item in fields.values() {
                    if walk(item, keys, depth + 1)? {
                        return Ok(true);
                    }
                }
                false
            }
            _ => false,
        })
    }
    if descriptor_target(intent.spec().name) == Some("hotkey_ss58")
        && intent
            .to_json_dyn()
            .get("hotkey_ss58")
            .is_none_or(Value::is_null)
        && keys.hotkey_descriptor().is_some()
    {
        return Ok(true);
    }
    walk(&intent.to_json_dyn(), keys, 0)
}

tokio::task_local! {
    static GUARDED: Vec<AccountId>;
}

/// Run `future` (an intent build) with these receiving-address accounts
/// marked as guarded by its composer.
///
/// [`with_recipient_registration`] (or its batch form) must then guard every
/// one of them. Outside such a scope an intent build naming a receiving
/// address refuses (see [`check_build_recipients`]).
pub async fn guarded_build<F: Future>(recipients: &[(String, Recipient)], future: F) -> F::Output {
    let mut accounts: Vec<AccountId> = GUARDED.try_with(Clone::clone).unwrap_or_default();
    accounts.extend(recipients.iter().map(|(_, recipient)| recipient.account));
    GUARDED.scope(accounts, future).await
}

/// Refuse an intent build naming a receiving address that no composer guards.
///
/// The semantic build resolves a receiving address to its account; only the
/// executor, a batch or a nested-intent composer adds the registration guard
/// that keeps the payment from reaching an unregistered account. Called by
/// the blanket [`DynIntent::build_dyn`].
pub fn check_build_recipients(intent: &dyn DynIntent) -> Result<(), IntentError> {
    let scoped = GUARDED.try_with(|_| ()).is_ok();
    if !scoped
        && intent
            .to_json_dyn()
            .get("hashed_descriptor")
            .is_some_and(|value| !value.is_null())
    {
        return Err(validation(format!(
            "{}: hashed_descriptor registers the recipient; submit it through the Executor, which adds the registration",
            intent.spec().name
        )));
    }
    let recipients = receiving_recipients(intent)?;
    if recipients.is_empty() {
        return Ok(());
    }
    let guarded = GUARDED.try_with(Clone::clone).unwrap_or_default();
    if let Some((name, _)) = recipients
        .iter()
        .find(|(_, recipient)| !guarded.contains(&recipient.account))
    {
        return Err(validation(format!(
            "{}: {name} is a receiving address; submit it through the Executor, which adds its registration guard",
            intent.spec().name
        )));
    }
    Ok(())
}

fn json_to_core(value: &Value) -> CoreValue {
    match value {
        Value::Null => CoreValue::Null,
        Value::Bool(value) => CoreValue::Bool(*value),
        Value::Number(number) => number
            .as_i64()
            .map(|value| CoreValue::Int(i128::from(value)))
            .or_else(|| {
                number
                    .as_u64()
                    .map(|value| CoreValue::Uint(u128::from(value)))
            })
            .unwrap_or_else(|| CoreValue::Str(number.to_string())),
        Value::String(text) => CoreValue::Str(text.clone()),
        Value::Array(items) => CoreValue::List(items.iter().map(json_to_core).collect()),
        Value::Object(fields) => CoreValue::record(
            fields
                .iter()
                .map(|(name, value)| (name.clone(), json_to_core(value)))
                .collect(),
        ),
    }
}

fn descriptor_call_value(descriptor: &[u8]) -> Result<rtao_core::DynValue, IntentError> {
    Ok(rtao_core::DynValue::new(json_to_core(
        &registration_descriptor_value(descriptor)?,
    )))
}

/// Python's `atomic_calls`: flatten only same-origin atomic batches
/// (`Utility.batch_all`), preserving every other wrapper.
#[doc(alias = "bittensor.hashed.atomic_calls")]
pub fn atomic_calls(
    runtime: &RuntimeHandle,
    call: CallBytes,
) -> Result<Vec<CallBytes>, IntentError> {
    use rtao_core::codec::decode::{Cursor, compact_len};
    let bytes = call.as_bytes();
    let (Some(pallet), Some(function)) = (
        runtime.pallet_index("Utility"),
        runtime.call_index("Utility", "batch_all"),
    ) else {
        return Ok(vec![call]);
    };
    if bytes.len() < 2 || bytes[0] != pallet || bytes[1] != function {
        return Ok(vec![call]);
    }
    let core = runtime.core();
    let mut cursor = Cursor::new(&bytes[2..]);
    let count = compact_len(&mut cursor).map_err(rtao_core::CoreError::from)?;
    let mut flattened = Vec::with_capacity(count);
    for _ in 0..count {
        let start = cursor.offset;
        core.decode_call_value(&mut cursor)
            .map_err(rtao_core::CoreError::from)?;
        let child = CallBytes::decode(&cursor.data[start..cursor.offset], runtime)?;
        flattened.extend(atomic_calls(runtime, child)?);
    }
    if cursor.offset != cursor.data.len() {
        return Err(validation("batch call has trailing bytes"));
    }
    Ok(flattened)
}

/// Materials for [`with_recipient_registration`].
pub struct RecipientCx<'a, V: View + ?Sized> {
    /// Pinned state the call is composed against.
    pub view: &'a V,
    /// Runtime for call construction.
    pub runtime: &'a RuntimeHandle,
    /// Public wallet keys: the default hotkey and local public descriptors
    /// (never a private key).
    pub keys: &'a dyn KeyView,
    /// Dispatch origin (the proxied or multisig account when wrapped).
    pub origin: OriginView<'a>,
    /// Finalized state, for operations that cannot carry a guard; `None`
    /// refuses them when a hashed recipient needs that check.
    pub finalized: Option<&'a (dyn rtao_client::DynView + 'a)>,
}

enum Guarded {
    /// No hashed recipient, or one checked in finalized state.
    Direct(CallBytes, Map<String, Value>),
    /// Registration guards and the operation's flattened calls.
    Atomic(Vec<CallBytes>, Map<String, Value>),
}

fn ss58(runtime: &RuntimeHandle, account: AccountId) -> String {
    account.to_ss58(runtime.core().ss58_format)
}

fn hex_descriptor(value: &str) -> Result<Vec<u8>, IntentError> {
    raw_bytes(&Value::String(value.to_owned()))
        .ok_or_else(|| validation("hashed_descriptor must be hex"))
}

async fn guard<V: View + ?Sized>(
    cx: &RecipientCx<'_, V>,
    intent: &dyn DynIntent,
    call: CallBytes,
    recipients: &[(String, Recipient)],
    as_calls: bool,
) -> Result<Guarded, IntentError> {
    let op = intent.spec().name;
    let args = intent.to_json_dyn();
    let mut recipients: Vec<(String, Recipient)> = recipients.to_vec();
    let set = |recipients: &mut Vec<(String, Recipient)>, name: &str, recipient: Recipient| {
        match recipients.iter_mut().find(|(field, _)| field == name) {
            Some((_, slot)) => *slot = recipient,
            None => recipients.push((name.to_owned(), recipient)),
        }
    };
    if op == "set_children" {
        for (index, pair) in args
            .get("children")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            let name = format!("children[{index}][1]");
            if recipients.iter().any(|(field, _)| *field == name) {
                continue;
            }
            if let Some(address) = pair
                .as_array()
                .and_then(|pair| pair.get(1))
                .and_then(Value::as_str)
                && let Ok(account) = address.parse::<AccountId>()
                && let Some(descriptor) = cx.keys.local_descriptor(&account)
            {
                set(
                    &mut recipients,
                    &name,
                    Recipient {
                        address: address.to_owned(),
                        account,
                        descriptor: Some(descriptor),
                    },
                );
            }
        }
    }
    let parameter = descriptor_target(op);
    if let Some(parameter) = parameter {
        let typed = recipients
            .iter()
            .find(|(field, _)| field == parameter)
            .map(|(_, recipient)| recipient.clone());
        let mut default_descriptor = None;
        let address = match args.get(parameter).and_then(Value::as_str) {
            Some(text) => Some(parse_recipient(text)?.account),
            // The wallet's own hotkey; without one attached the build has
            // already resolved its hotkey another way and nothing is known
            // about a descriptor for it.
            None if parameter == "hotkey_ss58" => match cx.keys.hotkey_address() {
                Ok(address) => {
                    default_descriptor = cx.keys.hotkey_descriptor();
                    Some(address)
                }
                Err(_) => None,
            },
            // An intent without the field has no recipient to guard; every
            // catalog intent naming a registration target requires it.
            None => None,
        };
        if let Some(address) = address {
            let explicit = args
                .get("hashed_descriptor")
                .and_then(Value::as_str)
                .map(hex_descriptor)
                .transpose()?;
            let descriptor = explicit
                .or_else(|| typed.as_ref().and_then(|typed| typed.descriptor.clone()))
                .or(default_descriptor)
                .or_else(|| cx.keys.local_descriptor(&address));
            if let Some(descriptor) = descriptor {
                let derived = descriptor_account(&descriptor)?;
                if derived != address
                    || typed.as_ref().is_some_and(|typed| {
                        typed.descriptor.as_deref() != Some(descriptor.as_slice())
                    })
                {
                    return Err(validation(
                        "hashed descriptor does not match the destination address",
                    ));
                }
                if is_stateless_pq_descriptor(&descriptor) {
                    require_receiving_support(cx.view, cx.runtime, &descriptor).await?;
                    recipients.retain(|(field, _)| field != parameter);
                } else {
                    let recipient = typed.unwrap_or(Recipient {
                        address: ss58(cx.runtime, address),
                        account: address,
                        descriptor: Some(descriptor),
                    });
                    set(&mut recipients, parameter, recipient);
                }
            }
        }
    }
    let mut kept = Vec::with_capacity(recipients.len());
    for (name, recipient) in recipients {
        if let Some(descriptor) = recipient.descriptor.as_deref()
            && is_stateless_pq_descriptor(descriptor)
        {
            if descriptor_account(descriptor)? != recipient.account {
                return Err(validation(
                    "hashed descriptor does not match the destination address",
                ));
            }
            continue;
        }
        kept.push((name, recipient));
    }
    let recipients = kept;
    if recipients.is_empty() {
        return Ok(if as_calls {
            Guarded::Atomic(atomic_calls(cx.runtime, call)?, Map::new())
        } else {
            Guarded::Direct(call, Map::new())
        });
    }
    let mut descriptors: BTreeMap<AccountId, Vec<u8>> = BTreeMap::new();
    for (_, recipient) in &recipients {
        let descriptor = recipient
            .descriptor
            .clone()
            .ok_or_else(|| validation("hashed recipient registration requires a descriptor"))?;
        if descriptor[0] == 1 {
            if hashed_accounts_enabled(cx.view, cx.runtime).await? != Some(true) {
                return Err(validation("hashed accounts are not enabled on this chain"));
            }
        } else {
            require_receiving_support(cx.view, cx.runtime, &descriptor).await?;
        }
        descriptors.insert(recipient.account, descriptor);
    }
    if op == "associate_hotkey" {
        let owner = cx.origin.coldkey_address()?;
        let kind = protected_account_kind(cx.view, cx.runtime, owner).await?;
        let requires_pq = descriptors.values().any(|descriptor| descriptor[0] == 2);
        if kind.is_none()
            || (requires_pq && !matches!(kind, Some(ProtectedKind::MlDsa | ProtectedKind::Derived)))
        {
            return Err(validation(
                "a hashed hotkey requires a registered hashed coldkey owner",
            ));
        }
    }
    let mut direct = matches!(
        op,
        "swap_coldkey_announced" | "pow_register" | "execute_proxy_announced"
    );
    if op == "announce_coldkey_swap"
        && let Some((_, destination)) = recipients
            .iter()
            .find(|(field, _)| field == "new_coldkey_ss58")
    {
        // An existing announcement locks out Utility.batch_all, even a
        // check-only one: keep first use atomic, but require finality and a
        // direct call once the destination is permanently registered.
        direct = registered_record(
            cx.view,
            destination.account,
            &descriptors[&destination.account],
        )
        .await?
        .is_some();
    }
    let operation = match op {
        "pow_register" => "PoW registration",
        "execute_proxy_announced" => "announced proxy execution",
        _ => "coldkey swap",
    };
    let finalized = if direct {
        if as_calls {
            return Err(validation(format!(
                "submit the {operation} directly, not inside a batch"
            )));
        }
        Some(cx.finalized.ok_or_else(|| {
            validation("could not verify finalized hashed destination registration")
        })?)
    } else {
        None
    };
    let mut unique: Vec<AccountId> = Vec::new();
    for (_, recipient) in &recipients {
        if !unique.contains(&recipient.account) {
            unique.push(recipient.account);
        }
    }
    let mut registrations = Vec::new();
    let mut new_accounts = Vec::new();
    let mut total_deposit = 0u64;
    for address in &unique {
        let descriptor = &descriptors[address];
        let record = match finalized {
            Some(view) => registered_record(view, *address, descriptor).await?,
            None => registered_record(cx.view, *address, descriptor).await?,
        };
        match &record {
            Some(stored) => {
                if stored != descriptor {
                    return Err(validation(
                        "registered hashed descriptor does not match the destination",
                    ));
                }
            }
            None => {
                if descriptor_registration_enabled(cx.view, cx.runtime, descriptor).await?
                    != Some(true)
                {
                    return Err(validation(
                        "new account registrations are not enabled on this chain",
                    ));
                }
                if finalized.is_some() {
                    return Err(validation(format!(
                        "hashed destination registration is not finalized; register the recipient and wait for finalization before executing the {operation}"
                    )));
                }
                let target_field = parameter.and_then(|parameter| {
                    recipients
                        .iter()
                        .find(|(field, _)| field == parameter)
                        .map(|(_, recipient)| recipient.account)
                });
                if !is_registration_target(op)
                    || target_field != Some(*address)
                    || unique.len() != 1
                {
                    return Err(validation(
                        "recipient is not registered; send it a direct payment before using this operation",
                    ));
                }
                new_accounts.push(*address);
                let legacy = if descriptor[0] == 2 {
                    &consts::ML_DSA_REGISTRATION_DEPOSIT
                } else {
                    &consts::REGISTRATION_DEPOSIT
                };
                let quote = quote_registration_deposit(cx.view, cx.runtime, legacy).await?;
                total_deposit = total_deposit
                    .checked_add(quote.amount_rao)
                    .ok_or(rtao_primitives::UnitError::Overflow)?;
                if quote.configurable {
                    registrations.push(
                        calls::check_registration_deposit(TaoBalance(quote.amount_rao))
                            .compose(cx.runtime)?,
                    );
                }
            }
        }
        if finalized.is_some() {
            continue;
        }
        let value = descriptor_call_value(descriptor)?;
        registrations.push(
            if record.is_some() {
                calls::check_registered(value)
            } else {
                calls::register(value)
            }
            .compose(cx.runtime)?,
        );
    }
    if let Some(view) = finalized {
        // Registrations cannot be removed or have their descriptor changed:
        // finality supplies the safety the guard otherwise provides.
        let hash = view.pinned().map(|block| block.hash).ok_or_else(|| {
            validation("could not verify finalized hashed destination registration")
        })?;
        return Ok(Guarded::Direct(
            call,
            Map::from_iter([(
                "hashed_registration_finalized_at".to_owned(),
                Value::from(format!("0x{}", hex::encode(hash.0))),
            )]),
        ));
    }
    // A check-only guard fails closed after a reorg without granting proxies
    // the authority to sponsor registration; only first use reserves.
    let mut guarded = registrations;
    guarded.extend(atomic_calls(cx.runtime, call)?);
    let mut extras = Map::new();
    extras.insert(
        "hashed_registration_guards".into(),
        Value::from(
            unique
                .iter()
                .map(|account| Value::from(ss58(cx.runtime, *account)))
                .collect::<Vec<_>>(),
        ),
    );
    extras.insert(
        "hashed_registration_max_deposit_rao".into(),
        Value::from(total_deposit),
    );
    if let Some(first) = new_accounts.first() {
        extras.insert(
            "hashed_registration".into(),
            Value::from(ss58(cx.runtime, *first)),
        );
        extras.insert(
            "hashed_registration_deposit_rao".into(),
            Value::from(total_deposit),
        );
    }
    Ok(Guarded::Atomic(guarded, extras))
}

/// The descriptor the account's own registry holds for it, if registered.
async fn registered_record<V: View + ?Sized>(
    view: &V,
    account: AccountId,
    descriptor: &[u8],
) -> Result<Option<Vec<u8>>, IntentError> {
    let record = if descriptor_storage(descriptor)? == "MlDsaAccounts" {
        view.storage(&st::ML_DSA_ACCOUNTS, (account,)).await?
    } else {
        view.storage(&st::ACCOUNTS, (account,)).await?
    };
    record
        .map(|record| {
            let record = record.to_json();
            let stored = record
                .get("descriptor")
                .ok_or_else(|| validation("HashedAccounts record has no descriptor"))?;
            Ok(descriptor_bytes(stored)?)
        })
        .transpose()
}

/// Python's `with_recipient_registration`: guard a known hashed recipient and
/// its operation in one atomic batch.
///
/// `call` is `intent`'s semantic call, before any origin wrapper; the caller
/// adds Sudo, proxy and multisig wrappers afterwards so setup stays under the
/// same authorization. The recipient is a receiving address in an address
/// field (`recipients`, from [`receiving_recipients`]), the intent's explicit
/// `hashed_descriptor`, the wallet's default hotkey or a local public key
/// file. A first payment registers it (`HashedAccounts.register`, after
/// `check_registration_deposit` where the reserve is configurable); a
/// registered one is checked (`check_registered`); either way in one
/// `Utility.batch_all` with the operation. A coldkey swap after its
/// announcement, PoW registration and announced proxy execution instead
/// require the registration in finalized state and stay direct.
///
/// The returned extras name the guards (`hashed_registration_guards`,
/// `hashed_registration_max_deposit_rao`) and, for a first use,
/// `hashed_registration` and `hashed_registration_deposit_rao`, from which
/// the executor quotes the sponsorship it must authorize before signing.
#[doc(alias = "bittensor.hashed.with_recipient_registration")]
pub async fn with_recipient_registration<V: View + ?Sized>(
    cx: &RecipientCx<'_, V>,
    intent: &dyn DynIntent,
    call: CallBytes,
    recipients: &[(String, Recipient)],
) -> Result<(CallBytes, Map<String, Value>), IntentError> {
    match guard(cx, intent, call, recipients, false).await? {
        Guarded::Direct(call, extras) => Ok((call, extras)),
        Guarded::Atomic(calls, extras) => {
            Ok((utility::batch_all(calls).compose(cx.runtime)?, extras))
        }
    }
}

/// [`with_recipient_registration`] for a batch child (Python's
/// `as_calls=True`): the guards and the child's flattened calls, for the
/// enclosing `Utility.batch_all` (the runtime rejects nested batches).
///
/// Operations that cannot carry a guard refuse here.
pub async fn recipient_registration_calls<V: View + ?Sized>(
    cx: &RecipientCx<'_, V>,
    intent: &dyn DynIntent,
    call: CallBytes,
    recipients: &[(String, Recipient)],
) -> Result<(Vec<CallBytes>, Map<String, Value>), IntentError> {
    match guard(cx, intent, call, recipients, true).await? {
        Guarded::Direct(call, extras) => Ok((vec![call], extras)),
        Guarded::Atomic(calls, extras) => Ok((calls, extras)),
    }
}

/// The deposit a call's metadata reserves for automatic setup, as a balance.
pub(crate) fn maximum_setup_deposit(extras: &Map<String, Value>) -> Balance<Tao> {
    Balance::from_rao(
        extras
            .get("hashed_registration_max_deposit_rao")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
    )
}
