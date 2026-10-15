//! Reproduce the committed 2,500-UID benchmark fixture from the recorded subnet.
use rtao_core::codec::{Value, decode::Cursor};
use rtao_primitives::AccountId;
use rtao_runtime::RuntimeHandle;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::env::args()
        .nth(1)
        .ok_or("provide fixture output path")?;
    let runtime = RuntimeHandle::parse(
        include_bytes!("../../../vendor/metadata/finney/478.v15.scale"),
        478,
        1,
        42,
    )?;
    let method = runtime
        .runtime_api("SubnetInfoRuntimeApi", "get_metagraph")
        .ok_or("get_metagraph absent")?;
    let mut cursor = Cursor::new(include_bytes!("../../../vendor/bench/finney-1.scale"));
    let mut value = runtime.core().decode_id(method.output, &mut cursor)?;
    let Value::Dict(fields) = &mut value else {
        return Err("metagraph absent".into());
    };
    for (name, value) in fields {
        let Value::Str(name) = name else {
            continue;
        };
        match name.as_str() {
            "num_uids" | "max_uids" => *value = Value::Int(2500),
            "hotkeys" | "coldkeys" => {
                *value = Value::List(
                    (0u32..2500)
                        .map(|uid| {
                            let mut bytes = [0; 32];
                            bytes
                                .get_mut(..4)
                                .ok_or("key slice absent")?
                                .copy_from_slice(&uid.to_le_bytes());
                            Ok(Value::Str(AccountId(bytes).to_string()))
                        })
                        .collect::<Result<_, Box<dyn std::error::Error>>>()?,
                );
            }
            "identities"
            | "axons"
            | "active"
            | "validator_permit"
            | "pruning_score"
            | "last_update"
            | "emission"
            | "dividends"
            | "incentives"
            | "consensus"
            | "trust"
            | "rank"
            | "block_at_registration"
            | "alpha_stake"
            | "tao_stake"
            | "total_stake"
            | "collateral_locked"
            | "collateral_min"
            | "collateral_earned" => {
                let Value::List(values) = value else {
                    return Err(format!("{name} is not a list").into());
                };
                if !values.is_empty() {
                    *values = values.iter().cycle().take(2500).cloned().collect();
                }
            }
            _ => {}
        }
    }
    let mut bytes = Vec::new();
    runtime
        .core()
        .encode_id(method.output, &value, &mut bytes)?;
    std::fs::write(output, bytes)?;
    Ok(())
}
