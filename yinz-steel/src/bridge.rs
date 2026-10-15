use anyhow::{bail, Context, Result};
use serde_json::Value;
use steel::gc::Gc;
use steel::rvals::SteelVal;

const MAX_DEPTH: usize = 64;
const MAX_ITEMS: usize = 100_000;
const MAX_BYTES: usize = 64 * 1024 * 1024;
// Host events can contain the complete session inventory, up to the IPC frame limit.
pub(crate) const MAX_INPUT_BYTES: usize = 64 * 1024 * 1024;
const MAX_INPUT_ITEMS: usize = 4 * 1024 * 1024;

struct Budget {
    items: usize,
    bytes: usize,
    max_items: usize,
    max_bytes: usize,
    binary_bytes: usize,
}
impl Budget {
    fn new(max_items: usize, max_bytes: usize) -> Self {
        Self {
            items: 0,
            bytes: 0,
            max_items,
            max_bytes,
            binary_bytes: 0,
        }
    }
}

impl Budget {
    fn visit(&mut self, depth: usize, bytes: usize) -> Result<()> {
        self.items += 1;
        self.bytes = self.bytes.saturating_add(bytes);
        anyhow::ensure!(
            depth <= MAX_DEPTH,
            "JSON bridge nesting exceeds {MAX_DEPTH}"
        );
        anyhow::ensure!(
            self.items <= self.max_items && self.bytes <= self.max_bytes,
            "JSON bridge value exceeds size limit"
        );
        Ok(())
    }
}

pub fn to_scheme(value: &Value) -> Result<SteelVal> {
    encode(
        value,
        0,
        &mut Budget::new(MAX_INPUT_ITEMS, MAX_INPUT_BYTES),
        &mut Vec::new(),
        None,
    )
}

pub(crate) fn to_typed(value: &Value, binary: fn(&[String]) -> bool) -> Result<SteelVal> {
    encode(
        value,
        0,
        &mut Budget::new(MAX_INPUT_ITEMS, MAX_INPUT_BYTES),
        &mut Vec::new(),
        Some(binary),
    )
}

fn encode(
    value: &Value,
    depth: usize,
    budget: &mut Budget,
    path: &mut Vec<String>,
    binary: Option<fn(&[String]) -> bool>,
) -> Result<SteelVal> {
    budget.visit(depth, 0)?;
    if binary.is_some_and(|binary| binary(path)) {
        let bytes = crate::binary::decode(value)?;
        budget.binary_bytes = budget.binary_bytes.saturating_add(bytes.len());
        anyhow::ensure!(
            budget.binary_bytes <= crate::binary::MAX_BYTES,
            "binary payload exceeds 64 MiB"
        );
        return Ok(SteelVal::ByteVector(steel::rvals::SteelByteVector::new(
            bytes,
        )));
    }
    Ok(match value {
        Value::Null => SteelVal::SymbolV("json-null".into()),
        Value::Bool(value) => SteelVal::BoolV(*value),
        Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                match isize::try_from(value) {
                    Ok(value) => SteelVal::IntV(value),
                    Err(_) => SteelVal::BigNum(Gc::new(value.into())),
                }
            } else if let Some(value) = value.as_u64() {
                match isize::try_from(value) {
                    Ok(value) => SteelVal::IntV(value),
                    Err(_) => SteelVal::BigNum(Gc::new(value.into())),
                }
            } else {
                SteelVal::NumV(value.as_f64().context("JSON number is not representable")?)
            }
        },
        Value::String(value) => {
            budget.visit(depth, value.len())?;
            SteelVal::StringV(value.as_str().into())
        },
        Value::Array(values) => SteelVal::ListV(
            values
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    if binary.is_some() {
                        path.push(index.to_string());
                    }
                    let result = encode(value, depth + 1, budget, path, binary);
                    if binary.is_some() {
                        path.pop();
                    }
                    result
                })
                .collect::<Result<Vec<_>>>()?
                .into(),
        ),
        Value::Object(values) => {
            let mut map = steel::HashMap::new();
            for (key, value) in values {
                budget.visit(depth, key.len())?;
                if binary.is_some() {
                    path.push(key.clone());
                }
                let value = encode(value, depth + 1, budget, path, binary)?;
                if binary.is_some() {
                    path.pop();
                }
                map.insert(SteelVal::StringV(key.as_str().into()), value);
            }
            SteelVal::HashMapV(Gc::new(map).into())
        },
    })
}

pub fn from_scheme(value: &SteelVal) -> Result<Value> {
    decode(
        value,
        0,
        &mut Budget::new(MAX_ITEMS, MAX_BYTES),
        &mut Vec::new(),
        false,
    )
}

pub(crate) fn from_model(value: &SteelVal) -> Result<Value> {
    decode(
        value,
        0,
        &mut Budget::new(MAX_INPUT_ITEMS, MAX_INPUT_BYTES),
        &mut Vec::new(),
        false,
    )
}

pub(crate) fn from_command(value: &SteelVal) -> Result<Value> {
    decode(
        value,
        0,
        &mut Budget::new(MAX_INPUT_ITEMS, MAX_BYTES),
        &mut Vec::new(),
        true,
    )
}

fn decode(
    value: &SteelVal,
    depth: usize,
    budget: &mut Budget,
    path: &mut Vec<String>,
    command: bool,
) -> Result<Value> {
    budget.visit(depth, 0)?;
    Ok(match value {
        SteelVal::SymbolV(value) if value.as_str() == "json-null" => Value::Null,
        SteelVal::BoolV(value) => Value::Bool(*value),
        SteelVal::IntV(value) => Value::from(*value as i64),
        SteelVal::BigNum(value) => {
            let number = value.to_string();
            if let Ok(signed) = number.parse::<i64>() {
                Value::from(signed)
            } else {
                Value::from(
                    number
                        .parse::<u64>()
                        .context("Scheme integer exceeds JSON integer range")?,
                )
            }
        },
        SteelVal::NumV(value) => {
            Value::Number(serde_json::Number::from_f64(*value).context("non-finite JSON number")?)
        },
        SteelVal::StringV(value) => {
            budget.visit(
                depth,
                if command {
                    value.encode_utf16().count().saturating_mul(6)
                } else {
                    value.len()
                },
            )?;
            Value::String(value.to_string())
        },
        SteelVal::ListV(values) => Value::Array(
            values
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    if command {
                        path.push(index.to_string());
                    }
                    let result = decode(value, depth + 1, budget, path, command);
                    if command {
                        path.pop();
                    }
                    result
                })
                .collect::<Result<_>>()?,
        ),
        SteelVal::VectorV(values) => Value::Array(
            values
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    if command {
                        path.push(index.to_string());
                    }
                    let result = decode(value, depth + 1, budget, path, command);
                    if command {
                        path.pop();
                    }
                    result
                })
                .collect::<Result<_>>()?,
        ),
        SteelVal::HashMapV(values) => {
            let mut map = serde_json::Map::new();
            for (key, value) in values.iter() {
                let SteelVal::StringV(key) = key else {
                    bail!("JSON object keys must be strings")
                };
                budget.visit(depth, key.len())?;
                if command {
                    path.push(key.to_string());
                }
                let value = decode(value, depth + 1, budget, path, command)?;
                if command {
                    path.pop();
                }
                map.insert(key.to_string(), value);
            }
            Value::Object(map)
        },
        SteelVal::ByteVector(bytes) => bytes.with_bytes(|bytes| {
            budget.visit(depth, bytes.len().saturating_mul(4))?;
            if command && crate::binary::command_byte_path(path) {
                crate::binary::encode(bytes)
            } else {
                anyhow::ensure!(
                    bytes.len() <= budget.max_items.saturating_sub(budget.items),
                    "JSON bridge value exceeds size limit"
                );
                budget.items += bytes.len();
                Ok(Value::Array(
                    bytes.iter().copied().map(Value::from).collect(),
                ))
            }
        })?,
        _ => bail!("Scheme value cannot cross the JSON bridge"),
    })
}
