// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/limit_orders.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/limit_orders.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
//! Owner authorization, cancellation and relay of complete limit orders.
//!
//! Orders are the runtime's `VersionedOrder` values as JSON (`{"V1": {...}}`,
//! `{"V2": {...}}`, or `{"Authorized": {"nonce": n, "terms": ...}}`), encoded
//! by the live metadata. Their identities (`signer`, `hotkey`,
//! `fee_recipient`, `relayer`) are ss58 or receiving (`tao_`) addresses.
//! `signer`, `hotkey` and `relayer` are identities: a receiving address is
//! replaced by its account. `fee_recipient` is paid: a receiving address keeps
//! its descriptor until its registration is checked in finalized state, and
//! only then becomes its account in the chain's order ([`order_for_chain`]).
//! Orders may be relayed much later and through either execution route; a
//! finalized registration is permanent, so it protects their fees without
//! changing the signed terms (D-144).

use rtao_client::{Client, View};
use rtao_core::{DynValue, codec::Value as CoreValue};
use rtao_primitives::receiving::{Recipient, parse_recipient};
use rtao_primitives::{AccountId, NetUid, Spend};
use rtao_runtime::{RuntimeHandle, calls::limit_orders as calls};
use rtao_tx::{BuildCx, Built, Intent, IntentError};
use serde_json::{Map, Value, json};
use smallvec::SmallVec;

pub use crate::{AuthorizeOrder, CancelOrder, ExecuteBatchedOrders, ExecuteOrders};

/// The domain of an authorized order's identifier.
pub const ORDER_ID_DOMAIN: &[u8] = b"bittensor/ms/v2/order-id";
/// Most account-approved orders one execution relays.
pub const MAX_AUTHORIZED_ORDERS: usize = 8;
const MAX_ORDER_DEPTH: usize = 32;

fn validation(message: impl Into<String>) -> IntentError {
    IntentError::validation(message)
}

// py: sdk/python/bittensor/limit_orders.py:21-22
fn hex_bytes(value: &str, label: &str) -> Result<Vec<u8>, IntentError> {
    hex::decode(value.strip_prefix("0x").unwrap_or(value))
        .map_err(|_| validation(format!("{label} must be hex bytes")))
}

// py: sdk/python/bittensor/limit_orders.py:25-28
fn u64_of(value: &Value, label: &str) -> Result<u64, IntentError> {
    value
        .as_u64()
        .filter(|_| value.is_number())
        .ok_or_else(|| validation(format!("{label} must be a u64 integer")))
}

// py: sdk/python/bittensor/limit_orders.py:31-49
/// The economic terms of a complete order, through one `Authorized` layer.
pub fn order_terms(order: &Value) -> Result<&Map<String, Value>, IntentError> {
    let object = order
        .as_object()
        .filter(|object| object.len() == 1)
        .ok_or_else(|| validation("order must contain one complete VersionedOrder variant"))?;
    if let Some(authorized) = object.get("Authorized") {
        let fields = authorized
            .as_object()
            .filter(|fields| {
                fields.len() == 2 && fields.contains_key("nonce") && fields.contains_key("terms")
            })
            .ok_or_else(|| validation("authorized order must contain nonce and complete terms"))?;
        u64_of(&fields["nonce"], "order nonce")?;
        let terms = &fields["terms"];
        if !terms.as_object().is_some_and(|terms| {
            terms.len() == 1 && (terms.contains_key("V1") || terms.contains_key("V2"))
        }) {
            return Err(validation(
                "authorized order terms must be a nonrecursive V1 or V2 variant",
            ));
        }
        return order_terms(terms);
    }
    let (variant, terms) = object
        .iter()
        .next()
        .ok_or_else(|| validation("order must contain one complete VersionedOrder variant"))?;
    if variant != "V1" && variant != "V2" {
        return Err(validation(
            "order terms must contain one complete V1 or V2 variant",
        ));
    }
    terms
        .as_object()
        .ok_or_else(|| validation("VersionedOrder terms must be an object"))
}

fn order_terms_mut(order: &mut Value) -> Option<&mut Map<String, Value>> {
    let object = order.as_object_mut()?;
    let inner = if object.contains_key("Authorized") {
        object
            .get_mut("Authorized")?
            .get_mut("terms")?
            .as_object_mut()?
    } else {
        object
    };
    inner.values_mut().next()?.as_object_mut()
}

pub use rtao_primitives::receiving::is_receiving_address;

// py: sdk/python/bittensor/receiving.py:32-43
fn recipient(value: &Value, field: &str) -> Result<Recipient, IntentError> {
    let text = value
        .as_str()
        .ok_or_else(|| validation("recipient must be an address string"))?;
    parse_recipient(text).map_err(|error| validation(format!("order {field}: {error}")))
}

/// An identity term: SS58 as written, a receiving address as its account.
fn identity(value: &Value, field: &str) -> Result<String, IntentError> {
    let recipient = recipient(value, field)?;
    Ok(match recipient.descriptor {
        None => recipient.address,
        Some(_) => recipient.account.to_string(),
    })
}

fn term_account(terms: &Map<String, Value>, field: &str) -> Result<AccountId, IntentError> {
    Ok(recipient(terms.get(field).unwrap_or(&Value::Null), field)?.account)
}

/// The order's `fee_recipient` when it is a receiving address still awaiting
/// its finalized registration check.
fn unresolved_fee_recipient(order: &Value) -> Result<Option<String>, IntentError> {
    let terms = order_terms(order)?;
    let recipient = recipient(
        terms.get("fee_recipient").unwrap_or(&Value::Null),
        "fee_recipient",
    )?;
    Ok(recipient.descriptor.map(|_| recipient.address))
}

fn needs_finalized_check(address: &str) -> IntentError {
    validation(format!(
        "order fee_recipient {address} is a receiving address, whose registration must be checked in finalized state before the order is encoded; prepare it with PreparedOrder::prepare_checked (or resolve it with limit_orders::order_for_chain)"
    ))
}

fn term_netuid(terms: &Map<String, Value>) -> Result<NetUid, IntentError> {
    let netuid = u64_of(terms.get("netuid").unwrap_or(&Value::Null), "order netuid")?;
    u16::try_from(netuid)
        .map(NetUid)
        .map_err(|_| validation("order netuid must be a u16 subnet"))
}

// py: sdk/python/bittensor/limit_orders.py:52-63
/// Normalize an order's identities, as Python's `canonical_order`: a
/// classical address is kept as written, an identity's receiving address
/// becomes its account, and a receiving `fee_recipient` keeps its descriptor
/// until [`order_for_chain`] checks its finalized registration.
pub fn canonical_order(order: &Value) -> Result<Value, IntentError> {
    let mut result = order.clone();
    let terms = order_terms(&result)?.clone();
    let canonical = order_terms_mut(&mut result)
        .ok_or_else(|| validation("VersionedOrder terms must be an object"))?;
    for field in ["signer", "hotkey"] {
        canonical.insert(
            field.into(),
            json!(identity(terms.get(field).unwrap_or(&Value::Null), field)?),
        );
    }
    let fee = recipient(
        terms.get("fee_recipient").unwrap_or(&Value::Null),
        "fee_recipient",
    )?;
    canonical.insert("fee_recipient".into(), json!(fee.address));
    if let Some(relayer) = terms.get("relayer").filter(|relayer| !relayer.is_null()) {
        let relayers = relayer
            .as_array()
            .ok_or_else(|| validation("order relayer must be a list of addresses"))?
            .iter()
            .map(|item| identity(item, "relayer").map(Value::String))
            .collect::<Result<Vec<_>, _>>()?;
        canonical.insert("relayer".into(), Value::Array(relayers));
    }
    Ok(result)
}

// py: sdk/python/bittensor/limit_orders.py:66-92
/// Python's `registered_fee_recipient`: a fee recipient's account, once a
/// receiving address's descriptor is permanently bound in finalized state.
#[doc(alias = "bittensor.limit_orders.registered_fee_recipient")]
pub async fn registered_fee_recipient(
    client: &Client,
    address: &str,
) -> Result<AccountId, IntentError> {
    let recipient = parse_recipient(address)?;
    if recipient.descriptor.is_none() {
        return Ok(recipient.account);
    }
    match rtao_tx::receiving::finalized_descriptor(client, &recipient).await? {
        None => Err(validation(
            "order fee recipient registration is not finalized; register the recipient and wait for finalization before preparing or executing orders",
        )),
        Some(stored) if Some(stored.as_slice()) != recipient.descriptor.as_deref() => Err(
            validation("registered order fee recipient descriptor does not match"),
        ),
        Some(_) => Ok(recipient.account),
    }
}

// py: sdk/python/bittensor/limit_orders.py:95-99
/// Python's `order_for_chain`: the canonical order with a receiving
/// `fee_recipient` replaced by its account after the finalized registration
/// check. An order without one needs no chain read.
#[doc(alias = "bittensor.limit_orders.order_for_chain")]
pub async fn order_for_chain<V: View + ?Sized>(
    view: &V,
    order: &Value,
) -> Result<Value, IntentError> {
    let mut result = canonical_order(order)?;
    if let Some(address) = unresolved_fee_recipient(&result)? {
        let snapshot = view.pin().await?;
        let account = registered_fee_recipient(snapshot.client(), &address).await?;
        order_terms_mut(&mut result)
            .ok_or_else(|| validation("VersionedOrder terms must be an object"))?
            .insert("fee_recipient".into(), json!(account.to_string()));
    }
    Ok(result)
}

// py: sdk/python/bittensor/limit_orders.py:103-107
/// Wrap complete economic terms for the account-aware calls.
pub fn authorized_order(nonce: u64, order: &Value) -> Result<Value, IntentError> {
    if order.get("Authorized").is_some() {
        return Err(validation(
            "provide V1 or V2 economic terms, not an already authorized order",
        ));
    }
    Ok(json!({"Authorized": {"nonce": nonce, "terms": canonical_order(order)?}}))
}

fn core_value(value: &Value, depth: usize) -> Result<CoreValue, IntentError> {
    if depth >= MAX_ORDER_DEPTH {
        return Err(validation("order JSON nests too deeply"));
    }
    Ok(match value {
        Value::Null => CoreValue::Null,
        Value::Bool(value) => CoreValue::Bool(*value),
        Value::String(value) => CoreValue::Str(value.clone()),
        Value::Number(number) => match (number.as_i64(), number.as_u64()) {
            (Some(value), _) => CoreValue::Int(i128::from(value)),
            (None, Some(value)) => CoreValue::Uint(u128::from(value)),
            _ => {
                return Err(validation(format!(
                    "order number {number} is not an integer"
                )));
            }
        },
        Value::Array(items) => CoreValue::List(
            items
                .iter()
                .map(|item| core_value(item, depth + 1))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(object) => CoreValue::Dict(
            object
                .iter()
                .map(|(key, value)| {
                    Ok((CoreValue::Str(key.clone()), core_value(value, depth + 1)?))
                })
                .collect::<Result<_, IntentError>>()?,
        ),
    })
}

fn dyn_value(value: &Value) -> Result<DynValue, IntentError> {
    Ok(DynValue::new(core_value(value, 0)?))
}

// py: sdk/python/bittensor/limit_orders.py:110-126
/// The runtime codec's exact SCALE encoding of the `VersionedOrder` terms an
/// authorized order with `nonce` carries.
///
/// This is synchronous, so it encodes only chain-ready terms: a receiving
/// `fee_recipient` is refused, naming [`order_for_chain`] and
/// [`PreparedOrder::prepare_checked`], which check its finalized registration.
pub fn encoded_order(
    runtime: &RuntimeHandle,
    nonce: u64,
    order: &Value,
) -> Result<Vec<u8>, IntentError> {
    if let Some(address) = unresolved_fee_recipient(&canonical_order(order)?)? {
        return Err(needs_finalized_check(&address));
    }
    let call =
        calls::authorize_order(dyn_value(&authorized_order(nonce, order)?)?).compose(runtime)?;
    let data = call.as_bytes();
    // Call indexes occupy two bytes, then Authorized's discriminant (2) and
    // the uncompressed u64 nonce.
    if data.len() <= 11 || data[2] != 2 || data[3..11] != nonce.to_le_bytes() {
        return Err(validation(
            "runtime returned an incompatible authorized order layout",
        ));
    }
    Ok(data[11..].to_vec())
}

// py: sdk/python/bittensor/limit_orders.py:129-151
/// An order prepared for authorization: its complete terms, their exact
/// encoding and the chain they belong to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedOrder {
    /// The owner-local nonce.
    pub nonce: u64,
    /// The complete V1 or V2 terms.
    pub order: Value,
    /// The terms' SCALE encoding.
    pub encoded: Vec<u8>,
    /// The chain's genesis hash.
    pub genesis_hash: [u8; 32],
}
impl PreparedOrder {
    /// Prepare terms with a nonce above the owner's
    /// `ApprovedOrders.highest_nonce`. The chain permits eight live approvals
    /// per owner; cancellation or expiry frees a slot but does not reset the
    /// nonce watermark.
    ///
    /// This needs no chain read, so it refuses a receiving-address
    /// `fee_recipient`, whose registration must be checked in finalized state:
    /// use [`Self::prepare_checked`] for those (and for any order, if preferred).
    pub fn prepare(
        runtime: &RuntimeHandle,
        genesis_hash: [u8; 32],
        nonce: u64,
        order: &Value,
    ) -> Result<Self, IntentError> {
        let order = canonical_order(order)?;
        if let Some(address) = unresolved_fee_recipient(&order)? {
            return Err(needs_finalized_check(&address));
        }
        Ok(Self {
            nonce,
            encoded: encoded_order(runtime, nonce, &order)?,
            order,
            genesis_hash,
        })
    }

    /// Python's `LimitOrders.prepare` (`prepare_order`): prepare terms against
    /// the client's chain, resolving a receiving-address `fee_recipient` to its
    /// account only once its registration is finalized ([`order_for_chain`]).
    ///
    /// The prepared order carries the chain-ready terms, so [`Self::check`],
    /// [`Self::authorize`] and [`Self::account_approved`] use the account.
    #[doc(alias = "bittensor.limit_orders.prepare_order")]
    pub async fn prepare_checked(
        client: &Client,
        nonce: u64,
        order: &Value,
    ) -> Result<Self, IntentError> {
        let snapshot = client.at(rtao_client::BlockRef::Best).await?;
        let order = order_for_chain(&snapshot, order).await?;
        Ok(Self {
            nonce,
            encoded: encoded_order(&snapshot.block().runtime, nonce, &order)?,
            order,
            genesis_hash: client.genesis().0,
        })
    }
    /// The order's owner, its `signer`.
    pub fn owner(&self) -> Result<AccountId, IntentError> {
        term_account(order_terms(&self.order)?, "signer")
    }
    /// BLAKE2b-256 of the domain, genesis, owner, nonce and encoded terms.
    pub fn order_id(&self) -> Result<[u8; 32], IntentError> {
        let mut payload = ORDER_ID_DOMAIN.to_vec();
        payload.extend_from_slice(&self.genesis_hash);
        payload.extend_from_slice(&self.owner()?.0);
        payload.extend_from_slice(&self.nonce.to_le_bytes());
        payload.extend_from_slice(&self.encoded);
        let hash = rtao_core::codec::storage::hash_param("Blake2_256", &payload)
            .map_err(rtao_core::CoreError::from)
            .map_err(|error| validation(format!("order id hash failed: {error}")))?;
        hash.try_into()
            .map_err(|_| validation("Blake2_256 returned an invalid hash length"))
    }
    /// Refuse when the chain or the terms' encoding changed since preparing.
    pub fn check(
        &self,
        runtime: &RuntimeHandle,
        genesis_hash: [u8; 32],
    ) -> Result<(), IntentError> {
        if genesis_hash != self.genesis_hash
            || encoded_order(runtime, self.nonce, &self.order)? != self.encoded
        {
            return Err(validation(
                "prepared order bytes or chain changed; prepare and authorize the complete order again",
            ));
        }
        Ok(())
    }
    /// The signed order an account approval relays.
    pub fn account_approved(&self, partial_fill: Option<u64>) -> Value {
        json!({
            "order": {"Authorized": {"nonce": self.nonce, "terms": self.order}},
            "signature": "AccountApproved",
            "partial_fill": partial_fill,
        })
    }
    /// The authorization intent for these terms, pinned to their encoding.
    pub fn authorize(&self) -> AuthorizeOrder {
        AuthorizeOrder {
            nonce: self.nonce,
            order: self.order.clone(),
            order_data: hex::encode(&self.encoded),
        }
    }
    /// The cancellation intent for these terms, pinned to their encoding.
    pub fn cancel(&self) -> CancelOrder {
        CancelOrder {
            nonce: self.nonce,
            order: self.order.clone(),
            order_data: hex::encode(&self.encoded),
        }
    }
}

// py: sdk/python/bittensor/intents/limit_orders.py:22-64
fn owner_validate(order: &Value, order_data: &str) -> Result<(), IntentError> {
    canonical_order(order)?;
    if !order_data.is_empty() {
        hex_bytes(order_data, "order_data")?;
    }
    Ok(())
}

fn owner_touches(order: &Value) -> SmallVec<[NetUid; 2]> {
    order_terms(order)
        .ok()
        .and_then(|terms| term_netuid(terms).ok())
        .into_iter()
        .collect()
}

async fn owner_build<V: View + ?Sized>(
    cx: &BuildCx<'_, V>,
    nonce: u64,
    order: &Value,
    order_data: &str,
    compose: fn(DynValue) -> rtao_runtime::Call,
) -> Result<Built, IntentError> {
    let order = canonical_order(order)?;
    let signer = term_account(order_terms(&order)?, "signer")?;
    let origin = cx.coldkey_address()?;
    if signer != origin {
        return Err(validation(
            "the order owner must authorize this call, directly or through its proxy/multisig origin",
        ));
    }
    // The chain's terms: a receiving fee recipient only after its finalized check.
    let order = order_for_chain(cx.view, &order).await?;
    if !order_data.is_empty()
        && encoded_order(cx.runtime, nonce, &order)? != hex_bytes(order_data, "order_data")?
    {
        return Err(validation(
            "pinned order bytes changed; prepare and authorize the complete order again",
        ));
    }
    Ok(Built::new(
        compose(dyn_value(&authorized_order(nonce, &order)?)?).compose(cx.runtime)?,
    ))
}

// py: sdk/python/bittensor/intents/limit_orders.py:67-86
impl Intent for AuthorizeOrder {
    fn summary(&self) -> String {
        format!("authorize complete order nonce {}", self.nonce)
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        owner_build(
            cx,
            self.nonce,
            &self.order,
            &self.order_data,
            calls::authorize_order,
        )
        .await
    }

    fn spend(&self) -> Spend {
        // Grants relayers the complete specified trade.
        Spend::Unbounded
    }

    fn touches_netuids(&self) -> SmallVec<[NetUid; 2]> {
        owner_touches(&self.order)
    }

    fn validate(&self) -> Result<(), IntentError> {
        owner_validate(&self.order, &self.order_data)
    }
}

// py: sdk/python/bittensor/intents/limit_orders.py:89-98
impl Intent for CancelOrder {
    fn summary(&self) -> String {
        format!(
            "cancel order nonce {} and its unfilled remainder",
            self.nonce
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        owner_build(
            cx,
            self.nonce,
            &self.order,
            &self.order_data,
            calls::cancel_order,
        )
        .await
    }

    fn touches_netuids(&self) -> SmallVec<[NetUid; 2]> {
        owner_touches(&self.order)
    }

    fn validate(&self) -> Result<(), IntentError> {
        owner_validate(&self.order, &self.order_data)
    }
}

// py: sdk/python/bittensor/intents/limit_orders.py:118-149
/// Validate signed orders and canonicalize their identities and signatures.
fn signed_orders(orders: &[Value]) -> Result<Vec<Value>, IntentError> {
    if orders.is_empty() {
        return Err(validation("execution requires at least one order"));
    }
    let mut authorized_count = 0;
    let mut result = Vec::with_capacity(orders.len());
    for signed in orders {
        let fields = signed
            .as_object()
            .filter(|fields| {
                fields.len() == 3
                    && ["order", "signature", "partial_fill"]
                        .iter()
                        .all(|key| fields.contains_key(*key))
            })
            .ok_or_else(|| {
                validation("each signed order must include order, signature, and partial_fill")
            })?;
        let order = canonical_order(&fields["order"])?;
        let partial_fill = &fields["partial_fill"];
        if !partial_fill.is_null() {
            u64_of(partial_fill, "partial fill")?;
        }
        let authorized = order.get("Authorized").is_some();
        authorized_count += usize::from(authorized);
        let authorization = &fields["signature"];
        let signature = if authorization == "AccountApproved"
            || authorization == &json!({"AccountApproved": null})
        {
            if !authorized {
                return Err(validation("account approval requires an Authorized order"));
            }
            authorization.clone()
        } else {
            let (scheme, value) = authorization
                .as_object()
                .filter(|object| object.len() == 1)
                .and_then(|object| object.iter().next())
                .filter(|(scheme, _)| matches!(scheme.as_str(), "Ed25519" | "Sr25519" | "Ecdsa"))
                .ok_or_else(|| validation("unknown order authorization"))?;
            if authorized {
                return Err(validation(
                    "classical signatures require original V1 or V2 orders",
                ));
            }
            let bytes = hex_bytes(
                value
                    .as_str()
                    .ok_or_else(|| validation("invalid classical order signature length"))?,
                "order signature",
            )?;
            if bytes.len() != if scheme == "Ecdsa" { 65 } else { 64 } {
                return Err(validation("invalid classical order signature length"));
            }
            json!({scheme: format!("0x{}", hex::encode(bytes))})
        };
        result.push(json!({"order": order, "signature": signature, "partial_fill": partial_fill}));
    }
    if authorized_count > MAX_AUTHORIZED_ORDERS {
        return Err(validation("authorized execution requires 1..8 orders"));
    }
    Ok(result)
}

/// Each signed order's terms as the chain takes them ([`order_for_chain`]).
async fn chain_orders<V: View + ?Sized>(
    view: &V,
    mut orders: Vec<Value>,
) -> Result<Vec<Value>, IntentError> {
    for signed in &mut orders {
        let order = order_for_chain(view, &signed["order"]).await?;
        signed["order"] = order;
    }
    Ok(orders)
}

fn orders_touch(orders: &[Value]) -> SmallVec<[NetUid; 2]> {
    let mut netuids = orders
        .iter()
        .filter_map(|signed| order_terms(signed.get("order")?).ok())
        .filter_map(|terms| term_netuid(terms).ok())
        .collect::<Vec<_>>();
    netuids.sort_unstable();
    netuids.dedup();
    netuids.into_iter().collect()
}

// py: sdk/python/bittensor/intents/limit_orders.py:101-170
impl Intent for ExecuteOrders {
    fn summary(&self) -> String {
        format!("relay {} authorized orders", self.orders.len())
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        let orders = chain_orders(cx.view, signed_orders(&self.orders)?).await?;
        Ok(Built::new(
            calls::execute_orders(
                orders.iter().map(dyn_value).collect::<Result<_, _>>()?,
                self.should_fail,
            )
            .compose(cx.runtime)?,
        ))
    }

    fn touches_netuids(&self) -> SmallVec<[NetUid; 2]> {
        orders_touch(&self.orders)
    }

    fn validate(&self) -> Result<(), IntentError> {
        signed_orders(&self.orders).map(|_| ())
    }
}

// py: sdk/python/bittensor/intents/limit_orders.py:173-201
impl Intent for ExecuteBatchedOrders {
    fn summary(&self) -> String {
        format!(
            "relay {} authorized orders through subnet {} netting",
            self.orders.len(),
            self.netuid
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        self.validate()?;
        let orders = chain_orders(cx.view, signed_orders(&self.orders)?).await?;
        Ok(Built::new(
            calls::execute_batched_orders(
                self.netuid,
                orders.iter().map(dyn_value).collect::<Result<_, _>>()?,
            )
            .compose(cx.runtime)?,
        ))
    }

    fn touches_netuids(&self) -> SmallVec<[NetUid; 2]> {
        orders_touch(&self.orders)
    }

    fn validate(&self) -> Result<(), IntentError> {
        signed_orders(&self.orders)?;
        let same_subnet = self.orders.iter().all(|signed| {
            signed
                .get("order")
                .and_then(|order| order_terms(order).ok())
                .and_then(|terms| term_netuid(terms).ok())
                == Some(self.netuid)
        });
        if self.netuid.0 == 0 || !same_subnet {
            return Err(validation(
                "all netted orders must target the same non-root subnet",
            ));
        }
        if !self.should_fail {
            return Err(validation("netted batches are atomic"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    const ALICE: &str = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";
    const BOB: &str = "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty";

    fn terms() -> Value {
        json!({"V1": {"signer": ALICE, "hotkey": BOB, "netuid": 3, "fee_recipient": BOB, "relayer": [BOB]}})
    }

    #[test]
    fn terms_are_found_through_one_authorized_layer() {
        assert_eq!(order_terms(&terms()).unwrap()["netuid"], 3);
        let authorized = authorized_order(7, &terms()).unwrap();
        assert_eq!(authorized["Authorized"]["nonce"], 7);
        assert_eq!(order_terms(&authorized).unwrap()["signer"], ALICE);
        assert!(authorized_order(7, &authorized).is_err());
        for bad in [
            json!({"V3": {}}),
            json!({"V1": {}, "V2": {}}),
            json!({"Authorized": {"nonce": 1}}),
            json!({"Authorized": {"nonce": 1, "terms": authorized}}),
            json!({"Authorized": {"nonce": -1, "terms": terms()}}),
        ] {
            assert!(order_terms(&bad).is_err(), "{bad}");
        }
    }

    /// Complete V1 terms, as Python's `test_mldsa_orders.order`.
    fn full_terms() -> Value {
        json!({"V1": {
            "signer": ALICE, "hotkey": BOB, "netuid": 1, "order_type": "LimitBuy",
            "amount": 100, "limit_price": 1_000_000_000_u64, "expiry": 2_000_000_000_000_u64,
            "fee_rate": 0, "fee_recipient": BOB, "relayer": [BOB], "max_slippage": 1000,
            "chain_id": 42, "partial_fills_enabled": true,
        }})
    }

    fn runtime() -> RuntimeHandle {
        RuntimeHandle::parse(
            include_bytes!("../../../vendor/metadata/finney/488.v15.scale"),
            488,
            1,
            42,
        )
        .unwrap()
    }

    fn hashed() -> (rtao_core::keys::Keypair, String) {
        let key =
            rtao_core::keys::Keypair::from_seed(&[31; 32], rtao_core::keys::CRYPTO_HASHED_ED25519)
                .unwrap();
        let address = key.hashed_receiving_address().unwrap();
        (key, address)
    }

    #[test]
    fn receiving_identities_resolve_and_receiving_fees_wait_for_finality() {
        let (key, address) = hashed();
        let mut order = full_terms();
        order["V1"]["signer"] = json!(address);
        order["V1"]["relayer"] = json!([address, BOB]);
        order["V1"]["fee_recipient"] = json!(address);
        let canonical = canonical_order(&order).unwrap();
        let terms = order_terms(&canonical).unwrap();
        // Identities are their accounts; the paid fee recipient keeps its descriptor.
        assert_eq!(terms["signer"], key.ss58_address());
        assert_eq!(terms["relayer"], json!([key.ss58_address(), BOB]));
        assert_eq!(terms["fee_recipient"], address);
        // Neither synchronous path encodes a receiving fee recipient.
        let runtime = runtime();
        for error in [
            PreparedOrder::prepare(&runtime, [0; 32], 1, &order).unwrap_err(),
            encoded_order(&runtime, 1, &order).unwrap_err(),
        ] {
            let error = error.to_string();
            assert!(
                error.contains("receiving address") && error.contains("prepare_checked"),
                "{error}"
            );
        }
        // A classical fee recipient is unchanged and needs no chain read.
        order["V1"]["fee_recipient"] = json!(BOB);
        let prepared = PreparedOrder::prepare(&runtime, [0; 32], 1, &order).unwrap();
        assert_eq!(prepared.owner().unwrap(), AccountId(key.public_key_bytes()));
        assert!(is_receiving_address("  BTH1x") && is_receiving_address("tao_x"));
        assert!(!is_receiving_address(ALICE));
        // A malformed receiving address or SS58 is refused while canonicalizing.
        let mut tampered = address.clone();
        tampered.pop();
        tampered.push(if address.ends_with('A') { 'B' } else { 'A' });
        for bad in [tampered, "not an address".to_owned()] {
            order["V1"]["fee_recipient"] = json!(bad);
            assert!(canonical_order(&order).is_err());
        }
    }

    #[tokio::test]
    async fn prepare_checked_resolves_only_a_finalized_matching_registration() -> anyhow::Result<()>
    {
        use rtao_client::{ClientConfig, Network};
        use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
        let (key, address) = hashed();
        let account = AccountId(key.public_key_bytes());
        let descriptor = key.hashed_descriptor()?;
        let mut order = full_terms();
        order["V1"]["fee_recipient"] = json!(address);
        let chain = FakeChain::from_state(FakeState::try_default()?);
        let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
        config.track_heads = false;
        config.runtime_cache_dir = None;
        config.config_store = None;
        let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
        let error = PreparedOrder::prepare_checked(&client, 4, &order)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("registration is not finalized"), "{error}");
        // Another account's descriptor registered under this account is refused.
        let (other, _) = {
            let key = rtao_core::keys::Keypair::from_seed(
                &[32; 32],
                rtao_core::keys::CRYPTO_HASHED_ED25519,
            )?;
            (key.hashed_descriptor()?, ())
        };
        let record = |descriptor: &[u8]| {
            json!({
                "descriptor": rtao_primitives::receiving::descriptor_value(descriptor).unwrap(),
                "generation": 0,
                "commitment": format!("0x{}", "11".repeat(32)),
            })
        };
        chain.mutate_state(|state| {
            state.seed_python(
                "HashedAccounts",
                "Accounts",
                &[json!(account)],
                record(&other),
            )
        })?;
        let error = PreparedOrder::prepare_checked(&client, 4, &order)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("descriptor does not match"), "{error}");
        chain.mutate_state(|state| {
            state.seed_python(
                "HashedAccounts",
                "Accounts",
                &[json!(account)],
                record(&descriptor),
            )
        })?;
        let prepared = PreparedOrder::prepare_checked(&client, 4, &order).await?;
        assert_eq!(
            order_terms(&prepared.order)?["fee_recipient"],
            account.to_string()
        );
        assert_eq!(prepared.genesis_hash, client.genesis().0);
        let runtime = rtao_client::View::runtime(&client).await?;
        prepared.check(&runtime, prepared.genesis_hash)?;
        assert_eq!(prepared.authorize().order, prepared.order);
        client.close().await;
        Ok(())
    }

    #[test]
    fn signed_orders_follow_python_authorization_rules() {
        let authorized = authorized_order(1, &terms()).unwrap();
        let approved =
            json!({"order": authorized, "signature": "AccountApproved", "partial_fill": null});
        assert!(signed_orders(std::slice::from_ref(&approved)).is_ok());
        let classical = json!({"order": terms(), "signature": {"Sr25519": format!("0x{}", "ab".repeat(64))}, "partial_fill": 5});
        let checked = signed_orders(std::slice::from_ref(&classical)).unwrap();
        assert_eq!(
            checked[0]["signature"]["Sr25519"],
            format!("0x{}", "ab".repeat(64))
        );
        for (bad, message) in [
            (vec![], "at least one order"),
            (
                vec![
                    json!({"order": terms(), "signature": "AccountApproved", "partial_fill": null}),
                ],
                "requires an Authorized order",
            ),
            (
                vec![
                    json!({"order": authorized_order(1, &terms()).unwrap(), "signature": {"Ed25519": format!("0x{}", "00".repeat(64))}, "partial_fill": null}),
                ],
                "original V1 or V2",
            ),
            (
                vec![
                    json!({"order": terms(), "signature": {"Ecdsa": format!("0x{}", "00".repeat(64))}, "partial_fill": null}),
                ],
                "signature length",
            ),
            (
                vec![
                    json!({"order": terms(), "signature": {"Other": "0x00"}, "partial_fill": null}),
                ],
                "unknown order authorization",
            ),
            (
                vec![json!({"order": terms(), "signature": "AccountApproved"})],
                "partial_fill",
            ),
            (vec![approved.clone(); 9], "1..8 orders"),
        ] {
            let error = signed_orders(&bad).unwrap_err().to_string();
            assert!(error.contains(message), "{error} lacks {message}");
        }
    }

    #[test]
    fn owner_orders_validate_summarize_and_scope_their_subnet() {
        let authorize = AuthorizeOrder {
            nonce: 9,
            order: terms(),
            order_data: String::new(),
        };
        assert!(authorize.validate().is_ok());
        assert_eq!(authorize.summary(), "authorize complete order nonce 9");
        assert_eq!(authorize.spend(), Spend::Unbounded);
        assert_eq!(authorize.touches_netuids().as_slice(), &[NetUid(3)]);
        let cancel = CancelOrder {
            nonce: 9,
            order: terms(),
            order_data: "0xzz".into(),
        };
        assert!(cancel.validate().unwrap_err().to_string().contains("hex"));
        let cancel = CancelOrder {
            order_data: String::new(),
            ..cancel
        };
        assert_eq!(
            cancel.summary(),
            "cancel order nonce 9 and its unfilled remainder"
        );
        assert_eq!(cancel.spend(), Spend::None);
        let relay = ExecuteOrders {
            orders: vec![
                json!({"order": authorized_order(1, &terms()).unwrap(), "signature": "AccountApproved", "partial_fill": null}),
                json!({"order": {"V1": {"signer": ALICE, "hotkey": BOB, "netuid": 1, "fee_recipient": BOB}}, "signature": {"Ed25519": format!("0x{}", "01".repeat(64))}, "partial_fill": 2}),
            ],
            should_fail: true,
        };
        assert!(relay.validate().is_ok());
        assert_eq!(relay.summary(), "relay 2 authorized orders");
        assert_eq!(relay.touches_netuids().as_slice(), &[NetUid(1), NetUid(3)]);
    }

    #[test]
    fn netted_batches_are_atomic_and_share_one_subnet() {
        let approved = json!({"order": authorized_order(1, &terms()).unwrap(), "signature": "AccountApproved", "partial_fill": null});
        let batch = |netuid: u16, should_fail: bool| ExecuteBatchedOrders {
            orders: vec![approved.clone()],
            should_fail,
            netuid: NetUid(netuid),
        };
        assert!(batch(3, true).validate().is_ok());
        assert!(batch(4, true).validate().is_err());
        assert!(
            batch(3, false)
                .validate()
                .unwrap_err()
                .to_string()
                .contains("atomic")
        );
        assert_eq!(batch(3, true).touches_netuids().as_slice(), &[NetUid(3)]);
    }
}
