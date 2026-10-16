// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/base.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};

use crate::{Alpha, Amount, Balance, DynBalance, Money, ResolvedAmount, Symbol, Tao};

fn money_schema(allow_all: bool) -> Schema {
    let mut variants = vec![
        serde_json::json!({ "type": "number", "minimum": 0 }),
        serde_json::json!({ "type": "string", "pattern": "^[0-9]+(\\.[0-9]{1,9})?$" }),
    ];
    if allow_all {
        variants.push(serde_json::json!({ "type": "string", "enum": ["all"] }));
    }
    json_schema!({ "anyOf": variants })
}

#[cfg_attr(docsrs, doc(cfg(feature = "schemars")))]
impl JsonSchema for Money {
    fn schema_name() -> Cow<'static, str> {
        "Money".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        money_schema(false)
    }
}

#[cfg_attr(docsrs, doc(cfg(feature = "schemars")))]
impl JsonSchema for Amount {
    fn schema_name() -> Cow<'static, str> {
        "Amount".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        money_schema(true)
    }
}

macro_rules! string_schema {
    ($($name:ty => $label:literal),+ $(,)?) => {$ (
        #[cfg_attr(docsrs, doc(cfg(feature = "schemars")))]
        impl JsonSchema for $name {
            fn schema_name() -> Cow<'static, str> { $label.into() }
            fn json_schema(generator: &mut SchemaGenerator) -> Schema { String::json_schema(generator) }
        }
    )+};
}
string_schema!(DynBalance => "DynBalance", Symbol => "Symbol", Balance<Tao> => "TaoAmount", Balance<Alpha> => "AlphaAmount", ResolvedAmount => "ResolvedAmount");
