//! Explicit command conversion: serde_v8's integer coercions are not a wire contract.
use anyhow::ensure;
use deno_core::v8;
use serde_json::Value;
use yinz_runtime::errors::prelude::*;

const MAX_DEPTH: usize = 128;
const MAX_BYTES: usize = yinz_ipc::MAX_IPC_FRAME_LEN;

pub(super) fn from_v8<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: v8::Local<'s, v8::Value>,
) -> Result<Value> {
    let mut remaining = MAX_BYTES;
    convert(scope, value, &mut Vec::new(), &mut remaining)
}

fn charge(remaining: &mut usize, bytes: usize) -> Result<()> {
    *remaining = remaining
        .checked_sub(bytes)
        .context("Deno command exceeds 64 MiB conversion budget")?;
    Ok(())
}

fn string(
    scope: &v8::PinScope,
    value: v8::Local<v8::String>,
    remaining: &mut usize,
) -> Result<String> {
    // Charge the worst-case JSON escape length before copying into Rust.
    charge(
        remaining,
        value
            .length()
            .checked_mul(6)
            .context("command string is too large")?,
    )?;
    Ok(value.to_rust_string_lossy(scope))
}

fn own_value<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    object: v8::Local<'s, v8::Object>,
    key: v8::Local<'s, v8::Name>,
) -> Result<v8::Local<'s, v8::Value>> {
    // The object was checked for Proxy before this call. A descriptor is a fresh
    // ordinary object; read only its own data slot, never the guest property.
    let descriptor = object
        .get_own_property_descriptor(scope, key)
        .context("cannot inspect command property")?;
    let descriptor = v8::Local::<v8::Object>::try_from(descriptor)
        .context("sparse command arrays are unsupported")?;
    let slot = v8::String::new(scope, "value").context("cannot allocate property name")?;
    ensure!(
        descriptor.has_own_property(scope, slot.into()) == Some(true),
        "command accessors are unsupported"
    );
    descriptor
        .get(scope, slot.into())
        .context("cannot read command data property")
}

fn convert<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: v8::Local<'s, v8::Value>,
    ancestors: &mut Vec<v8::Local<'s, v8::Object>>,
    remaining: &mut usize,
) -> Result<Value> {
    ensure!(
        ancestors.len() <= MAX_DEPTH,
        "Deno command exceeds nesting limit of 128"
    );
    charge(remaining, std::mem::size_of::<Value>())?;
    if value.is_null() {
        return Ok(Value::Null);
    }
    if value.is_boolean() {
        return Ok(Value::Bool(value.boolean_value(scope)));
    }
    if let Ok(value) = v8::Local::<v8::Number>::try_from(value) {
        let n = value.value();
        ensure!(n.is_finite(), "command numbers must be finite");
        return Ok(if n.fract() == 0.0 && n.abs() <= 9_007_199_254_740_991.0 {
            Value::from(n as i64)
        } else {
            // Retain the float tag: Rust integer destinations must reject it.
            Value::from(n)
        });
    }
    if let Ok(value) = v8::Local::<v8::BigInt>::try_from(value) {
        if let (n, true) = value.i64_value() {
            return Ok(Value::from(n));
        }
        if let (n, true) = value.u64_value() {
            return Ok(Value::from(n));
        }
        bail!("command BigInt is outside i64/u64 range");
    }
    if let Ok(value) = v8::Local::<v8::String>::try_from(value) {
        return string(scope, value, remaining).map(Value::String);
    }
    ensure!(!value.is_proxy(), "command proxies are unsupported");
    if let Ok(bytes) = v8::Local::<v8::Uint8Array>::try_from(value) {
        let length = bytes.byte_length();
        charge(
            remaining,
            length
                // A byte needs at most "255," on the wire. Charging Rust's
                // expanded Value storage would reject valid 8 MiB file writes.
                .checked_mul(4)
                .context("command byte array is too large")?,
        )?;
        let mut data = vec![0; length];
        ensure!(
            bytes.copy_contents(&mut data) == length,
            "cannot read command byte array"
        );
        return Ok(Value::Array(data.into_iter().map(Value::from).collect()));
    }
    let object = v8::Local::<v8::Object>::try_from(value).context("unsupported command value")?;
    let array = v8::Local::<v8::Array>::try_from(value).ok();
    if array.is_none() {
        let prototype = object
            .get_prototype(scope)
            .context("cannot inspect command prototype")?;
        let plain = v8::Object::new(scope)
            .get_prototype(scope)
            .context("cannot inspect data prototype")?;
        ensure!(
            prototype.is_null() || prototype.strict_equals(plain),
            "commands require plain data objects"
        );
        ensure!(!value.is_function(), "command functions are unsupported");
    }
    ensure!(
        !ancestors.iter().any(|parent| parent.strict_equals(value)),
        "cyclic command value"
    );
    ancestors.push(object);
    let keys = object
        .get_own_property_names(
            scope,
            v8::GetPropertyNamesArgs {
                property_filter: v8::PropertyFilter::ALL_PROPERTIES,
                key_conversion: v8::KeyConversionMode::ConvertToString,
                ..Default::default()
            },
        )
        .context("cannot inspect command keys")?;
    charge(
        remaining,
        (keys.length() as usize)
            .checked_mul(std::mem::size_of::<Value>())
            .context("too many command keys")?,
    )?;
    let mut fields = serde_json::Map::new();
    let mut elements = Vec::new();
    if let Some(array) = array {
        ensure!(
            array.length() as usize <= *remaining / std::mem::size_of::<Value>(),
            "Deno command array exceeds conversion budget"
        );
    }
    for index in 0..keys.length() {
        let key = keys
            .get_index(scope, index)
            .context("cannot inspect command key")?;
        let key = v8::Local::<v8::String>::try_from(key)
            .context("command symbol keys are unsupported")?;
        let name = string(scope, key, remaining)?;
        let item = own_value(scope, object, key.into())?;
        if array.is_some() && name == "length" {
            continue;
        }
        if array.is_some() {
            ensure!(
                name == elements.len().to_string(),
                "command arrays must have only dense indexed data"
            );
            elements.push(convert(scope, item, ancestors, remaining)?);
        } else {
            fields.insert(name, convert(scope, item, ancestors, remaining)?);
        }
    }
    ancestors.pop();
    if let Some(array) = array {
        ensure!(
            elements.len() == array.length() as usize,
            "sparse command arrays are unsupported"
        );
        Ok(Value::Array(elements))
    } else {
        Ok(Value::Object(fields))
    }
}

#[cfg(test)]
#[path = "unit/transport.rs"]
mod tests;
