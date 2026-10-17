//! Operator policy over trusted-runtime call decoding, and the client allowlist.

use std::collections::{BTreeMap, BTreeSet};

use rtao_btauth::{
    BoxFuture, Caller,
    server::{Admission, CallerPolicy, PolicyDenied},
};
use rtao_core::{codec::Value, runtime::TypeSpec};
use rtao_primitives::{AccountId, Balance, NetUid, Spend, Tao};
use rtao_runtime::RuntimeHandle;
use rtao_tx::{Policy, PolicySubject};
use smallvec::SmallVec;

/// Default and largest default mortal period accepted by a server.
pub const DEFAULT_MAX_ERA_PERIOD: u64 = 128;

/// Operator limits applied after a request's views are verified.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemotePolicy {
    /// Spend, fee, subnet and raw-call limits shared with the intent executor.
    pub limits: Policy,
    /// Allowed `(pallet, call)` pairs; `None` allows any call the limits accept.
    pub allowed_calls: Option<BTreeSet<(String, String)>>,
    /// Allowed transfer destinations; when set, only modeled transfers pass.
    pub allowed_destinations: Option<BTreeSet<AccountId>>,
    /// Longest accepted mortal period; a power of two in `4..=65536`.
    pub max_era_period: u64,
    /// Accept immortal transactions.
    pub allow_immortal: bool,
    /// Accept `shield_inner` and `shield_carrier` purposes.
    pub allow_shielded: bool,
    /// Require CheckMetadataHash with a digest of the trusted runtime.
    pub require_metadata_hash: bool,
}

impl Default for RemotePolicy {
    fn default() -> Self {
        Self {
            limits: Policy::default(),
            allowed_calls: None,
            allowed_destinations: None,
            max_era_period: DEFAULT_MAX_ERA_PERIOD,
            allow_immortal: false,
            allow_shielded: false,
            require_metadata_hash: false,
        }
    }
}

/// The policy view of one decoded call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedCallSubject {
    op: &'static str,
    spend: Spend,
    raw: bool,
}

impl PolicySubject for DecodedCallSubject {
    fn op(&self) -> &'static str {
        self.op
    }
    fn spend(&self) -> Spend {
        self.spend
    }
    fn touches_netuids(&self) -> SmallVec<[NetUid; 2]> {
        SmallVec::new()
    }
    fn affects_all_subnets(&self) -> bool {
        false
    }
    fn is_raw(&self) -> bool {
        self.raw
    }
}

/// A call decoded with the trusted runtime and classified for policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedCall {
    /// Pallet name.
    pub module: String,
    /// Call name.
    pub function: String,
    /// `MultiAddress::Id` destination of a modeled transfer.
    pub dest: Option<AccountId>,
    /// Policy subject.
    pub subject: DecodedCallSubject,
}

impl DecodedCall {
    /// `Pallet.call` label.
    pub fn label(&self) -> String {
        format!("{}.{}", self.module, self.function)
    }
    /// Whether the call is a modeled transfer.
    pub fn is_transfer(&self) -> bool {
        !self.subject.raw
    }
}

fn field<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    match value {
        Value::Dict(entries) => entries.iter().find_map(|(key, value)| match key {
            Value::Str(key) if key == name => Some(value),
            _ => None,
        }),
        _ => None,
    }
}

fn text(value: &Value) -> Option<&str> {
    match value {
        Value::Str(text) => Some(text),
        _ => None,
    }
}

fn unsigned(value: &Value) -> Option<u128> {
    match value {
        Value::Int(value) => u128::try_from(*value).ok(),
        Value::Uint(value) => Some(*value),
        _ => None,
    }
}

/// Decode the call's outer variant and classify it.
///
/// Only `Balances.transfer_keep_alive`, `transfer_allow_death` and
/// `transfer_all` with a `MultiAddress::Id` destination are modeled; every
/// other call, including wrappers such as `Proxy.proxy`, `Utility.batch` and
/// `Sudo.sudo`, is raw.
pub fn classify_call(runtime: &RuntimeHandle, call: &[u8]) -> Result<DecodedCall, String> {
    let value = runtime
        .core()
        .decode_spec(&TypeSpec::Call, call, true)
        .map_err(|error| error.to_string())?;
    let module = field(&value, "call_module")
        .and_then(text)
        .ok_or("decoded call has no module")?
        .to_owned();
    let function = field(&value, "call_function")
        .and_then(text)
        .ok_or("decoded call has no function")?
        .to_owned();
    let args = match field(&value, "call_args") {
        Some(Value::List(args)) => args.as_slice(),
        _ => return Err("decoded call has no argument list".into()),
    };
    let arg = |name: &str| {
        args.iter()
            .find(|arg| field(arg, "name").and_then(text) == Some(name))
            .and_then(|arg| field(arg, "value"))
    };
    let raw = DecodedCallSubject {
        op: "raw_call",
        spend: Spend::None,
        raw: true,
    };
    let modeled = match (module.as_str(), function.as_str()) {
        ("Balances", "transfer_keep_alive" | "transfer_allow_death") => Some((
            "transfer",
            arg("value")
                .and_then(unsigned)
                .and_then(|value| u64::try_from(value).ok())
                .map_or(Spend::Unbounded, |rao| {
                    Spend::Bounded(Balance::<Tao>::from_rao(rao))
                }),
        )),
        ("Balances", "transfer_all") => Some(("transfer_all", Spend::Unbounded)),
        _ => None,
    };
    // A modeled transfer's first argument is `dest: MultiAddress`. Only the
    // `Id` variant (wire byte 0x00 after the two call index bytes) maps to an
    // account; the decoded rendering must agree with the raw key bytes.
    let dest = modeled.and_then(|_| {
        let first = args.first()?;
        if field(first, "name").and_then(text) != Some("dest") || call.get(2) != Some(&0) {
            return None;
        }
        let key: [u8; 32] = call.get(3..35)?.try_into().ok()?;
        let decoded: AccountId = field(first, "value").and_then(text)?.parse().ok()?;
        (decoded.0 == key).then_some(decoded)
    });
    let subject = match (modeled, dest) {
        (Some((op, spend)), Some(_)) => DecodedCallSubject {
            op,
            spend,
            raw: false,
        },
        _ => raw,
    };
    Ok(DecodedCall {
        module,
        function,
        dest,
        subject,
    })
}

impl RemotePolicy {
    /// Check call allowlists, destinations and limits, in that order.
    ///
    /// A raw (unmodeled) call has no measurable spend, fee or subnet set, so
    /// it is refused while any of those limits is configured, even when
    /// `allow_raw_calls` is set. [`RemoteSignerServer::new`] already refuses
    /// such configurations; this keeps direct callers fail-closed.
    ///
    /// [`RemoteSignerServer::new`]: super::RemoteSignerServer::new
    pub fn evaluate(&self, call: &DecodedCall, fee: Option<Balance<Tao>>) -> Vec<String> {
        let mut violations = Vec::new();
        if let Some(allowed) = &self.allowed_calls
            && !allowed.contains(&(call.module.clone(), call.function.clone()))
        {
            violations.push(format!("call {} is not in allowed_calls", call.label()));
        }
        if let Some(allowed) = &self.allowed_destinations {
            match call.dest {
                Some(dest) if call.is_transfer() => {
                    if !allowed.contains(&dest) {
                        violations
                            .push(format!("destination {dest} is not in allowed_destinations"));
                    }
                }
                _ => violations.push(format!(
                    "call {} has no destination that allowed_destinations can check",
                    call.label()
                )),
            }
        }
        if call.subject.raw && self.limits.allow_raw_calls && self.has_measured_limits() {
            violations.push(format!(
                "call {} cannot be measured against max_spend_tao, max_fee_tao or allowed_netuids; raw calls are refused while those limits are set",
                call.label()
            ));
        }
        violations.extend(self.limits.check(&call.subject, fee));
        violations
    }

    fn has_measured_limits(&self) -> bool {
        self.limits.max_spend.is_some()
            || self.limits.max_fee.is_some()
            || self.limits.allowed_netuids.is_some()
    }

    /// Refuse limits the server cannot enforce.
    ///
    /// `allowed_netuids` is refused outright: no decoded call is modeled with
    /// a subnet, so the allowlist could never apply. `allow_raw_calls` is
    /// refused together with `max_spend` or `max_fee`, because a raw call
    /// (for example `Utility.batch_all` of transfers or `SubtensorModule.add_stake`)
    /// has no spend or fee the policy can measure and would bypass them.
    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        if self.limits.allowed_netuids.is_some() {
            return Err(
                "allowed_netuids is not supported by the remote signer: no decoded call is modeled with a subnet, so the limit could never apply; use allowed_calls",
            );
        }
        if self.limits.allow_raw_calls && self.has_measured_limits() {
            return Err(
                "allow_raw_calls cannot be combined with max_spend or max_fee: raw calls have no spend or fee the policy can measure, so they would bypass those limits",
            );
        }
        let period = self.max_era_period;
        if !(4..=65_536).contains(&period) || !period.is_power_of_two() {
            return Err("max_era_period must be a power of two between 4 and 65536");
        }
        Ok(())
    }
}

/// btauth admission: only configured client keys reach the replay store.
#[derive(Clone, Debug, Default)]
pub struct ClientAllowlist {
    grants: BTreeMap<AccountId, BTreeSet<AccountId>>,
}

impl ClientAllowlist {
    /// Admit exactly these client keys.
    pub fn new(grants: BTreeMap<AccountId, BTreeSet<AccountId>>) -> Self {
        Self { grants }
    }
    /// Accounts granted to a client.
    pub fn grants(&self, client: &AccountId) -> Option<&BTreeSet<AccountId>> {
        self.grants.get(client)
    }
}

impl CallerPolicy for ClientAllowlist {
    fn admit<'a>(&'a self, caller: &'a Caller) -> BoxFuture<'a, Result<Admission, PolicyDenied>> {
        Box::pin(async move {
            if self.grants.contains_key(&caller.hotkey) {
                Ok(Admission::default())
            } else {
                Err(PolicyDenied("client key is not authorized".into()))
            }
        })
    }
}
