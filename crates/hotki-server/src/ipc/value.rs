use mrpc::Value;
use serde::{Serialize, de::DeserializeOwned};

use crate::{Error, Result};

/// Encode a UTF-8 string parameter for MRPC requests.
pub(crate) fn string_param(value: &str) -> Value {
    Value::String(value.into())
}

/// Encode a typed payload as msgpack binary.
pub(crate) fn binary_param<T: Serialize>(value: &T) -> Result<Value> {
    let bytes = rmp_serde::to_vec_named(value)?;
    Ok(Value::Binary(bytes))
}

/// Decode a msgpack binary response into a typed value.
pub(crate) fn binary_response<T: DeserializeOwned>(value: Value, context: &str) -> Result<T> {
    match value {
        Value::Binary(bytes) => {
            rmp_serde::from_slice::<T>(&bytes).map_err(|err| Error::Serialization(err.to_string()))
        }
        other => Err(Error::Ipc(format!(
            "Unexpected {} response: {:?}",
            context, other
        ))),
    }
}

/// Decode an array of UTF-8 strings.
pub(crate) fn string_vec_response(value: Value, context: &str) -> Result<Vec<String>> {
    match value {
        Value::Array(values) => values
            .into_iter()
            .map(|value| string_response(value, context))
            .collect(),
        other => Err(Error::Ipc(format!(
            "Unexpected {} response: {:?}",
            context, other
        ))),
    }
}

/// Decode an integer response as `usize`.
pub(crate) fn usize_response(value: Value, context: &str) -> Result<usize> {
    match value {
        Value::Integer(raw) => raw
            .as_u64()
            .map(|value| value as usize)
            .ok_or_else(|| Error::Ipc(format!("Invalid {} value", context))),
        other => Err(Error::Ipc(format!(
            "Unexpected {} response: {:?}",
            context, other
        ))),
    }
}

fn string_response(value: Value, context: &str) -> Result<String> {
    match value {
        Value::String(raw) => raw
            .as_str()
            .map(|value| value.to_string())
            .ok_or_else(|| Error::Ipc(format!("Unexpected non-utf8 string in {}", context))),
        other => Err(Error::Ipc(format!(
            "Unexpected element in {}: {:?}",
            context, other
        ))),
    }
}
