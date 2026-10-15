// Derived from RaoFoundation/subtensor pallets/subtensor/src/lib.rs @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Serving addresses and identity wire projections.

use super::{ToDynamic, bytes_dynamic};
use rtao_core::codec::Value;
use scale_decode::DecodeAsType;

/// Runtime `AxonInfo` record.
#[derive(Clone, Debug, PartialEq, Eq, DecodeAsType, serde::Serialize, serde::Deserialize)]
pub struct AxonInfo {
    /// Runtime field `block`.
    pub block: u64,
    /// Runtime field `version`.
    pub version: u32,
    /// Runtime field `ip`.
    pub ip: u128,
    /// Runtime field `port`.
    pub port: u16,
    /// Runtime field `ip_type`.
    pub ip_type: u8,
    /// Runtime field `protocol`.
    pub protocol: u8,
    /// Runtime field `placeholder1`.
    pub placeholder1: u8,
    /// Runtime field `placeholder2`.
    pub placeholder2: u8,
}

impl Default for AxonInfo {
    fn default() -> Self {
        Self {
            block: 0,
            version: 0,
            ip: 0,
            port: 0,
            ip_type: 4,
            protocol: 4,
            placeholder1: 0,
            placeholder2: 0,
        }
    }
}

impl AxonInfo {
    /// Served HTTP endpoint, absent when the chain IP is zero or invalid.
    pub fn base_url(&self) -> Option<rtao_primitives::axon::Url> {
        rtao_primitives::axon::axon_base_url(self.ip, self.ip_type, self.port)
    }
}

impl ToDynamic for AxonInfo {
    fn to_dynamic(&self) -> Value {
        Value::record(vec![
            ("block".into(), self.block.to_dynamic()),
            ("version".into(), self.version.to_dynamic()),
            ("ip".into(), self.ip.to_dynamic()),
            ("port".into(), self.port.to_dynamic()),
            ("ip_type".into(), self.ip_type.to_dynamic()),
            ("protocol".into(), self.protocol.to_dynamic()),
            ("placeholder1".into(), self.placeholder1.to_dynamic()),
            ("placeholder2".into(), self.placeholder2.to_dynamic()),
        ])
    }
}

/// Runtime `ChainIdentityV2` record.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, DecodeAsType, serde::Serialize, serde::Deserialize,
)]
pub struct ChainIdentityV2 {
    /// Runtime field `name`.
    pub name: Vec<u8>,
    /// Runtime field `url`.
    pub url: Vec<u8>,
    /// Runtime field `github_repo`.
    pub github_repo: Vec<u8>,
    /// Runtime field `image`.
    pub image: Vec<u8>,
    /// Runtime field `discord`.
    pub discord: Vec<u8>,
    /// Runtime field `description`.
    pub description: Vec<u8>,
    /// Runtime field `additional`.
    pub additional: Vec<u8>,
}

impl ToDynamic for ChainIdentityV2 {
    fn to_dynamic(&self) -> Value {
        Value::record(vec![
            ("name".into(), bytes_dynamic(&self.name)),
            ("url".into(), bytes_dynamic(&self.url)),
            ("github_repo".into(), bytes_dynamic(&self.github_repo)),
            ("image".into(), bytes_dynamic(&self.image)),
            ("discord".into(), bytes_dynamic(&self.discord)),
            ("description".into(), bytes_dynamic(&self.description)),
            ("additional".into(), bytes_dynamic(&self.additional)),
        ])
    }
}

/// Runtime `SubnetIdentityV3` record.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, DecodeAsType, serde::Serialize, serde::Deserialize,
)]
pub struct SubnetIdentityV3 {
    /// Runtime field `subnet_name`.
    pub subnet_name: Vec<u8>,
    /// Runtime field `github_repo`.
    pub github_repo: Vec<u8>,
    /// Runtime field `subnet_contact`.
    pub subnet_contact: Vec<u8>,
    /// Runtime field `subnet_url`.
    pub subnet_url: Vec<u8>,
    /// Runtime field `discord`.
    pub discord: Vec<u8>,
    /// Runtime field `description`.
    pub description: Vec<u8>,
    /// Runtime field `logo_url`.
    pub logo_url: Vec<u8>,
    /// Runtime field `additional`.
    pub additional: Vec<u8>,
}

impl ToDynamic for SubnetIdentityV3 {
    fn to_dynamic(&self) -> Value {
        Value::record(vec![
            ("subnet_name".into(), bytes_dynamic(&self.subnet_name)),
            ("github_repo".into(), bytes_dynamic(&self.github_repo)),
            ("subnet_contact".into(), bytes_dynamic(&self.subnet_contact)),
            ("subnet_url".into(), bytes_dynamic(&self.subnet_url)),
            ("discord".into(), bytes_dynamic(&self.discord)),
            ("description".into(), bytes_dynamic(&self.description)),
            ("logo_url".into(), bytes_dynamic(&self.logo_url)),
            ("additional".into(), bytes_dynamic(&self.additional)),
        ])
    }
}

impl rtao_primitives::IntoCoreValue for AxonInfo {
    fn into_core_value(self) -> Value {
        Value::record(vec![
            (
                "block".into(),
                rtao_primitives::IntoCoreValue::into_core_value(self.block),
            ),
            (
                "version".into(),
                rtao_primitives::IntoCoreValue::into_core_value(self.version),
            ),
            (
                "ip".into(),
                rtao_primitives::IntoCoreValue::into_core_value(self.ip),
            ),
            (
                "port".into(),
                rtao_primitives::IntoCoreValue::into_core_value(self.port),
            ),
            (
                "ip_type".into(),
                rtao_primitives::IntoCoreValue::into_core_value(self.ip_type),
            ),
            (
                "protocol".into(),
                rtao_primitives::IntoCoreValue::into_core_value(self.protocol),
            ),
            (
                "placeholder1".into(),
                rtao_primitives::IntoCoreValue::into_core_value(self.placeholder1),
            ),
            (
                "placeholder2".into(),
                rtao_primitives::IntoCoreValue::into_core_value(self.placeholder2),
            ),
        ])
    }
}

impl rtao_primitives::IntoCoreValue for ChainIdentityV2 {
    fn into_core_value(self) -> Value {
        Value::record(vec![
            (
                "name".into(),
                rtao_primitives::IntoCoreValue::into_core_value(self.name),
            ),
            (
                "url".into(),
                rtao_primitives::IntoCoreValue::into_core_value(self.url),
            ),
            (
                "github_repo".into(),
                rtao_primitives::IntoCoreValue::into_core_value(self.github_repo),
            ),
            (
                "image".into(),
                rtao_primitives::IntoCoreValue::into_core_value(self.image),
            ),
            (
                "discord".into(),
                rtao_primitives::IntoCoreValue::into_core_value(self.discord),
            ),
            (
                "description".into(),
                rtao_primitives::IntoCoreValue::into_core_value(self.description),
            ),
            (
                "additional".into(),
                rtao_primitives::IntoCoreValue::into_core_value(self.additional),
            ),
        ])
    }
}

impl rtao_primitives::IntoCoreValue for SubnetIdentityV3 {
    fn into_core_value(self) -> Value {
        Value::record(vec![
            (
                "subnet_name".into(),
                rtao_primitives::IntoCoreValue::into_core_value(self.subnet_name),
            ),
            (
                "github_repo".into(),
                rtao_primitives::IntoCoreValue::into_core_value(self.github_repo),
            ),
            (
                "subnet_contact".into(),
                rtao_primitives::IntoCoreValue::into_core_value(self.subnet_contact),
            ),
            (
                "subnet_url".into(),
                rtao_primitives::IntoCoreValue::into_core_value(self.subnet_url),
            ),
            (
                "discord".into(),
                rtao_primitives::IntoCoreValue::into_core_value(self.discord),
            ),
            (
                "description".into(),
                rtao_primitives::IntoCoreValue::into_core_value(self.description),
            ),
            (
                "logo_url".into(),
                rtao_primitives::IntoCoreValue::into_core_value(self.logo_url),
            ),
            (
                "additional".into(),
                rtao_primitives::IntoCoreValue::into_core_value(self.additional),
            ),
        ])
    }
}
