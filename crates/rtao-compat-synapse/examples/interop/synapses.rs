//! Rust mirrors of `ci/interop/synapse/common.py`.

#![allow(dead_code)]

use rtao_compat_synapse::{PyDict, Synapse, tensor::Tensor};
use serde::{Deserialize, Serialize};

fn legacy_fields() -> Option<Vec<String>> {
    Some(vec!["b".into(), "a".into(), "d".into()])
}

/// `Mixed(Synapse)` with six hash fields.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mixed {
    pub i: i64,
    pub f: f64,
    pub s: String,
    pub l: Vec<String>,
    pub d: PyDict<String, Vec<f64>>,
    pub n: Option<i64>,
    pub out: Option<String>,
}
impl Synapse for Mixed {
    const NAME: &'static str = "Mixed";
    const REQUIRED_HASH_FIELDS: &'static [&'static str] = &["i", "f", "s", "l", "d", "n"];
}

/// `Req(Synapse)` with six required fields.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Req {
    pub rs: String,
    pub ri: i64,
    pub rl: Vec<i64>,
    pub rd: PyDict<String, i64>,
    pub rf: f64,
    pub rb: bool,
    #[serde(default)]
    pub echo: Option<String>,
}
impl Synapse for Req {
    const NAME: &'static str = "Req";
    const REQUIRED_HASH_FIELDS: &'static [&'static str] = &["rs", "ri", "rl", "rd", "rf", "rb"];
    const REQUIRED_FIELDS: &'static [&'static str] = &["rs", "ri", "rl", "rd", "rf", "rb"];
}

/// Deprecated pydantic-field `required_hash_fields` form. pydantic keeps the
/// redeclared field at the base class's position, before subclass fields.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LegacyHashed {
    pub required_hash_fields: Option<Vec<String>>,
    pub a: i64,
    pub b: i64,
    pub d: Option<Vec<String>>,
}
impl Default for LegacyHashed {
    fn default() -> Self {
        Self {
            required_hash_fields: legacy_fields(),
            a: 0,
            b: 0,
            d: None,
        }
    }
}
impl Synapse for LegacyHashed {
    const NAME: &'static str = "LegacyHashed";
    // The field default ["b", "a", "d"] re-sorted into model_dump order.
    const REQUIRED_HASH_FIELDS: &'static [&'static str] = &["a", "b", "d"];
}

/// Optional tensor plus its sum.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tens {
    pub t: Option<Tensor>,
    pub total: Option<f64>,
}
impl Synapse for Tens {
    const NAME: &'static str = "Tens";
    const REQUIRED_HASH_FIELDS: &'static [&'static str] = &["t"];
}

macro_rules! empty {
    ($($name:ident),*) => {$(
        #[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
        pub struct $name {}
        impl Synapse for $name {
            const NAME: &'static str = stringify!($name);
        }
    )*};
}
empty!(Deny, Status404, Boom);

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Slow {
    pub delay: f64,
}
impl Synapse for Slow {
    const NAME: &'static str = "Slow";
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NanResp {
    pub v: Option<f64>,
}
impl Synapse for NanResp {
    const NAME: &'static str = "NanResp";
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WhoAmI {
    pub seen: Option<String>,
}
impl Synapse for WhoAmI {
    const NAME: &'static str = "WhoAmI";
}

/// Non-finite request floats, which only Python dendrites send.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NanReq {
    pub v: f64,
    pub vs: Vec<f64>,
    pub seen: Option<String>,
}
impl Synapse for NanReq {
    const NAME: &'static str = "NanReq";
    const REQUIRED_HASH_FIELDS: &'static [&'static str] = &["v", "vs"];
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Echo {
    pub text: String,
    pub completion: String,
}
impl Synapse for Echo {
    const NAME: &'static str = "Echo";
}

/// The interop axon: every synapse above with `common.py`'s behaviours.
pub fn build_axon(
    signer: std::sync::Arc<dyn rtao_signer::Signer>,
    config: rtao_compat_synapse::V10AxonConfig,
) -> rtao_compat_synapse::Axon {
    use std::time::Duration;

    use futures::StreamExt;
    use rtao_compat_synapse::{Axon, DefaultHooks, Envelope, Reply, SynapseError};

    Axon::new(signer, config)
        .expect("valid axon configuration")
        .attach::<Mixed, _, _, _>(
            DefaultHooks,
            |_, mut envelope: Envelope<Mixed>| async move {
                let body = &mut envelope.body;
                body.out = Some(format!("{}|{}|{}", body.i, body.s, body.l.len()));
                Ok(Reply::Synapse(envelope))
            },
        )
        .attach::<Req, _, _, _>(DefaultHooks, |_, mut envelope: Envelope<Req>| async move {
            envelope.body.echo = Some(envelope.body.rs.clone());
            Ok(Reply::Synapse(envelope))
        })
        .attach::<LegacyHashed, _, _, _>(DefaultHooks, |_, envelope| async move {
            Ok(Reply::Synapse(envelope))
        })
        .attach::<Tens, _, _, _>(DefaultHooks, |_, mut envelope: Envelope<Tens>| async move {
            if let Some(tensor) = &envelope.body.t {
                let raw = tensor.to_raw().map_err(|_| SynapseError::internal())?;
                if raw.type_str != "<f4" {
                    return Err(SynapseError::new(400, "expected float32"));
                }
                let total: f32 = raw
                    .data
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|chunk| f32::from_le_bytes(*chunk))
                    .sum();
                envelope.body.total = Some(f64::from(total));
            }
            Ok(Reply::Synapse(envelope))
        })
        .attach::<Deny, _, _, _>(DenyHooks, |_, envelope| async move {
            Ok(Reply::Synapse(envelope))
        })
        .attach::<Status404, _, _, _>(
            DefaultHooks,
            |_, mut envelope: Envelope<Status404>| async move {
                if let Some(axon) = envelope.axon.as_mut() {
                    axon.status_code = Some(404);
                }
                Ok(Reply::Synapse(envelope))
            },
        )
        .attach::<Boom, _, _, _>(DefaultHooks, |_, _: Envelope<Boom>| async move {
            Err(SynapseError::internal())
        })
        .attach::<Slow, _, _, _>(DefaultHooks, |_, envelope: Envelope<Slow>| async move {
            tokio::time::sleep(Duration::from_secs_f64(
                envelope.body.delay.clamp(0.0, 60.0),
            ))
            .await;
            Ok(Reply::Synapse(envelope))
        })
        .attach::<NanResp, _, _, _>(
            DefaultHooks,
            |_, mut envelope: Envelope<NanResp>| async move {
                envelope.body.v = Some(f64::NAN);
                Ok(Reply::Synapse(envelope))
            },
        )
        .attach::<WhoAmI, _, _, _>(
            DefaultHooks,
            |_, mut envelope: Envelope<WhoAmI>| async move {
                envelope.body.seen = envelope.dendrite.as_ref().and_then(|d| d.hotkey.clone());
                Ok(Reply::Synapse(envelope))
            },
        )
        .attach::<Echo, _, _, _>(DefaultHooks, |_, envelope: Envelope<Echo>| async move {
            let chunks =
                futures::stream::iter(std::iter::repeat_n(bytes::Bytes::from_static(b"tok\n"), 3));
            Ok(Reply::Stream {
                head: envelope,
                body: chunks.boxed(),
            })
        })
        .attach::<NanReq, _, _, _>(
            DefaultHooks,
            |_, mut envelope: Envelope<NanReq>| async move {
                // Python: repr((v, vs)), then clear the non-finite values.
                let body = &mut envelope.body;
                let seen = rtao_compat_synapse::to_py_value(&(body.v, body.vs.clone()))
                    .map_err(|_| SynapseError::internal())?;
                body.seen = Some(rtao_compat_synapse::py_repr(&seen));
                body.v = 0.0;
                body.vs.clear();
                Ok(Reply::Synapse(envelope))
            },
        )
}

struct DenyHooks;
impl rtao_compat_synapse::Hooks<Deny> for DenyHooks {
    fn blacklist<'a>(
        &'a self,
        _head: &'a rtao_compat_synapse::RequestHead,
    ) -> futures::future::BoxFuture<'a, Result<(), String>> {
        Box::pin(async { Err("denied by test".to_owned()) })
    }
}
