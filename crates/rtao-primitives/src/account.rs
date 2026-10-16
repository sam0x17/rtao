use std::{fmt, str::FromStr};

use parity_scale_codec::{Decode, Encode};
use scale_decode::{IntoVisitor, TypeResolver};
use scale_info::TypeInfo;

use crate::decoding::{FromWire, WireVisitor};

/// A Substrate account, displayed and parsed through the core's SS58 codec.
#[derive(
    Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Encode, Decode, TypeInfo,
)]
pub struct AccountId(
    /// The exact account public-key bytes.
    pub [u8; 32],
);

impl AccountId {
    /// The all-zero account used by ValueQuery defaults.
    pub const ZERO: Self = Self([0; 32]);
    /// Borrow the exact account bytes.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
    /// Render SS58 with an explicitly requested network prefix.
    pub fn to_ss58(self, prefix: u16) -> String {
        rtao_core::keys::ss58_from_public(self.0, prefix)
    }
}

impl fmt::Display for AccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_ss58(rtao_core::keys::DEFAULT_SS58_FORMAT))
    }
}

impl FromStr for AccountId {
    type Err = rtao_core::CoreError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        rtao_core::keys::public_key_from_ss58(value)
            .map(Self)
            .map_err(Into::into)
    }
}

impl From<[u8; 32]> for AccountId {
    fn from(value: [u8; 32]) -> Self {
        Self(value)
    }
}

impl AsRef<[u8]> for AccountId {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl FromWire for AccountId {
    type Wire = [u8; 32];
    fn from_wire(value: Self::Wire) -> Result<Self, scale_decode::Error> {
        Ok(Self(value))
    }
}

impl IntoVisitor for AccountId {
    type AnyVisitor<R: TypeResolver> = WireVisitor<Self, R>;
    fn into_visitor<R: TypeResolver>() -> Self::AnyVisitor<R> {
        WireVisitor::new()
    }
}

#[cfg(feature = "serde")]
#[cfg_attr(docsrs, doc(cfg(feature = "serde")))]
impl serde::Serialize for AccountId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

#[cfg(feature = "serde")]
#[cfg_attr(docsrs, doc(cfg(feature = "serde")))]
impl<'de> serde::Deserialize<'de> for AccountId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(feature = "schemars")]
#[cfg_attr(docsrs, doc(cfg(feature = "schemars")))]
impl schemars::JsonSchema for AccountId {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "AccountId".into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        String::json_schema(generator)
    }
}

/// A validated SS58 input retaining the caller's exact address spelling.
///
/// Intent JSON and summaries preserve the supplied network prefix. Chain calls
/// use [`Self::account`] and the decoded public-key bytes, independent of it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ss58Address {
    spelling: String,
    account: AccountId,
}

impl Ss58Address {
    /// Borrow the unchanged, validated input string.
    pub fn as_str(&self) -> &str {
        &self.spelling
    }
    /// Return the decoded chain identity without changing the input spelling.
    pub const fn account(&self) -> AccountId {
        self.account
    }
    /// Borrow the public-key bytes used by the chain encoder.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        self.account.as_bytes()
    }
}

impl FromStr for Ss58Address {
    type Err = rtao_core::CoreError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::try_from(value.to_owned())
    }
}
impl TryFrom<String> for Ss58Address {
    type Error = rtao_core::CoreError;
    fn try_from(spelling: String) -> Result<Self, Self::Error> {
        let account = spelling.parse()?;
        Ok(Self { spelling, account })
    }
}
impl TryFrom<&str> for Ss58Address {
    type Error = rtao_core::CoreError;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        value.parse()
    }
}
impl From<AccountId> for Ss58Address {
    fn from(account: AccountId) -> Self {
        Self {
            spelling: account.to_string(),
            account,
        }
    }
}
impl AsRef<str> for Ss58Address {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}
impl fmt::Display for Ss58Address {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}
#[cfg(feature = "serde")]
#[cfg_attr(docsrs, doc(cfg(feature = "serde")))]
impl serde::Serialize for Ss58Address {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}
#[cfg(feature = "serde")]
#[cfg_attr(docsrs, doc(cfg(feature = "serde")))]
impl<'de> serde::Deserialize<'de> for Ss58Address {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::try_from(<String as serde::Deserialize>::deserialize(deserializer)?)
            .map_err(serde::de::Error::custom)
    }
}
#[cfg(feature = "schemars")]
#[cfg_attr(docsrs, doc(cfg(feature = "schemars")))]
impl schemars::JsonSchema for Ss58Address {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Ss58Address".into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        String::json_schema(generator)
    }
}

/// A ValueQuery account retaining its zero-account JSON representation.
#[derive(
    Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Encode, Decode, TypeInfo,
)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(transparent)
)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct DefaultAccount(AccountId);

impl FromWire for DefaultAccount {
    type Wire = [u8; 32];
    fn from_wire(value: Self::Wire) -> Result<Self, scale_decode::Error> {
        Ok(Self::new(AccountId(value)))
    }
}

impl IntoVisitor for DefaultAccount {
    type AnyVisitor<R: TypeResolver> = WireVisitor<Self, R>;
    fn into_visitor<R: TypeResolver>() -> Self::AnyVisitor<R> {
        WireVisitor::new()
    }
}

impl DefaultAccount {
    /// Wrap a raw storage account, including its all-zero default.
    pub const fn new(value: AccountId) -> Self {
        Self(value)
    }
    /// Return no owner for the all-zero account in the typed API.
    pub fn get(&self) -> Option<&AccountId> {
        (self.0 != AccountId::ZERO).then_some(&self.0)
    }
    /// Borrow the account used by the unchanged Python-shaped JSON output.
    pub const fn raw(&self) -> &AccountId {
        &self.0
    }
}

impl From<AccountId> for DefaultAccount {
    fn from(value: AccountId) -> Self {
        Self::new(value)
    }
}

impl fmt::Display for DefaultAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Signature and keypair algorithm codes shared with the core and MultiSignature.
#[derive(
    Copy, Clone, Debug, PartialEq, Eq, Hash, Encode, Decode, TypeInfo, scale_decode::DecodeAsType,
)]
#[repr(u8)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub enum CryptoType {
    /// Ed25519, with core and SCALE discriminant zero.
    Ed25519 = 0,
    /// Sr25519, with core and SCALE discriminant one.
    Sr25519 = 1,
}

impl From<CryptoType> for u8 {
    fn from(value: CryptoType) -> Self {
        value as Self
    }
}

impl TryFrom<u8> for CryptoType {
    type Error = crate::UnitError;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Ed25519),
            1 => Ok(Self::Sr25519),
            _ => Err(crate::UnitError::InvalidIdentifier {
                kind: "crypto type",
                value: u64::from(value),
                limit: 2,
            }),
        }
    }
}
