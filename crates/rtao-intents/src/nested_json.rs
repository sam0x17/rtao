//! Bounded programmatic JSON shared by nested semantic intent constructors.

use rtao_tx::IntentError;
use serde_json::Value;
use std::io::{self, Write};

pub(crate) const MAX_CALL_DEPTH: usize = 128;
const MAX_CALL_NODES: usize = 1 << 20;
const MAX_CALL_JSON_BYTES: usize = 64 * 1024 * 1024;

/// Check the owned JSON tree before recursive serde cloning or nested builds.
///
/// The traversal itself is iterative. At most one bounded tree is accepted;
/// serialized size is counted into a writer rather than another allocation.
pub(crate) fn check_call_budget(value: &Value) -> Result<(), IntentError> {
    check_call_limits(value, MAX_CALL_DEPTH, MAX_CALL_NODES, MAX_CALL_JSON_BYTES)
}

pub(crate) fn check_call_limits(
    value: &Value,
    max_depth: usize,
    max_nodes: usize,
    max_bytes: usize,
) -> Result<(), IntentError> {
    let cost = inspect_call_limits(value, max_depth, max_nodes, max_bytes)?;
    // Nested semantic builds retain their owned arguments while the next
    // subtree is cloned. Tree levels bound the number of retained copies;
    // charge every level the entire input rather than assuming a shape.
    cost.nodes
        .checked_mul(cost.levels)
        .filter(|work| *work <= max_nodes)
        .ok_or_else(|| {
            IntentError::validation("nested intent exceeds its recursive JSON node-work budget")
        })?;
    cost.bytes
        .checked_mul(cost.levels)
        .filter(|work| *work <= max_bytes)
        .ok_or_else(|| {
            IntentError::validation("nested intent exceeds its recursive JSON byte-work budget")
        })?;
    Ok(())
}

#[derive(Debug)]
pub(crate) struct CallCost {
    pub(crate) nodes: usize,
    pub(crate) levels: usize,
    pub(crate) bytes: usize,
}

pub(crate) fn inspect_call_limits(
    value: &Value,
    max_depth: usize,
    max_nodes: usize,
    max_bytes: usize,
) -> Result<CallCost, IntentError> {
    let mut remaining = max_nodes;
    let mut levels = 0;
    let mut stack = vec![(value, 0usize)];
    while let Some((value, depth)) = stack.pop() {
        remaining = remaining
            .checked_sub(1)
            .ok_or_else(|| IntentError::validation("nested intent exceeds its JSON node budget"))?;
        if depth >= max_depth {
            return Err(IntentError::validation(
                "nested intent exceeds its JSON depth budget",
            ));
        }
        levels = levels.max(depth + 1);
        match value {
            Value::Object(fields) => {
                if fields.len() > remaining.saturating_sub(stack.len()) {
                    return Err(IntentError::validation(
                        "nested intent exceeds its JSON node budget",
                    ));
                }
                for (key, child) in fields {
                    // Value's enabled private serde carriers must not introduce
                    // an embedded document after the bounded traversal.
                    if matches!(
                        key.as_str(),
                        "$serde_json::private::RawValue" | "$serde_json::private::Number"
                    ) {
                        return Err(IntentError::validation(
                            "private JSON carrier keys are not nested intent fields",
                        ));
                    }
                    stack.push((child, depth + 1));
                }
            }
            Value::Array(items) => {
                if items.len() > remaining.saturating_sub(stack.len()) {
                    return Err(IntentError::validation(
                        "nested intent exceeds its JSON node budget",
                    ));
                }
                stack.extend(items.iter().map(|child| (child, depth + 1)));
            }
            _ => {}
        }
    }
    let mut writer = SizeBudget(max_bytes);
    serde_json::to_writer(&mut writer, value)
        .map_err(|_| IntentError::validation("nested intent exceeds its JSON byte budget"))?;
    Ok(CallCost {
        nodes: max_nodes - remaining,
        levels,
        bytes: max_bytes - writer.0,
    })
}

struct SizeBudget(usize);
impl Write for SizeBudget {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self.0.checked_sub(bytes.len()).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "nested intent JSON size limit")
        })?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
