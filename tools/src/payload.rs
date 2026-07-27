use serde_json::Value;

use crate::ToolError;

pub(crate) fn parse_payload(input: &str) -> Result<Value, ToolError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(Value::Null);
    }
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return Ok(serde_json::from_str(trimmed)?);
    }
    Ok(Value::String(trimmed.to_string()))
}

pub(crate) fn field_or_raw(payload: &Value, key: &str, raw: &str) -> Result<String, ToolError> {
    field_or_raw_keys(payload, &[key], raw)
}

pub(crate) fn field_or_raw_keys(
    payload: &Value,
    keys: &[&str],
    raw: &str,
) -> Result<String, ToolError> {
    string_field_keys(payload, keys)
        .or_else(|| first_string_from_arrays(payload, keys))
        .or_else(|| raw_string_payload_or_input(payload, raw))
        .ok_or_else(|| ToolError::InvalidInput(format!("missing `{}`", keys[0])))
}

pub(crate) fn required_field_keys(
    payload: &Value,
    keys: &[&str],
    raw: &str,
) -> Result<String, ToolError> {
    field_or_raw_keys(payload, keys, raw)
}

pub(crate) fn string_field(payload: &Value, key: &str) -> Option<String> {
    payload.get(key).and_then(Value::as_str).map(str::to_string)
}

pub(crate) fn string_field_any(payload: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| string_field(payload, key))
}

pub(crate) fn string_field_keys(payload: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| string_field(payload, key))
        .and_then(|value| meaningful_string(&value))
}

pub(crate) fn exact_string_field_keys(payload: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| payload.get(key).and_then(Value::as_str).map(str::to_string))
}

fn first_string_from_arrays(payload: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        payload
            .get(key)
            .and_then(Value::as_array)
            .and_then(|items| {
                items.iter().find_map(|item| {
                    item.as_str()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string)
                })
            })
            .and_then(|value| meaningful_string(&value))
    })
}

fn raw_string_value(payload: &Value) -> Option<String> {
    match payload {
        Value::String(value) => meaningful_string(value),
        Value::Null => None,
        _ => None,
    }
}

pub(crate) fn raw_string_payload_or_input(payload: &Value, raw: &str) -> Option<String> {
    raw_string_value(payload).or_else(|| match payload {
        Value::Null => raw_string_input(raw),
        _ => None,
    })
}

fn raw_string_input(raw: &str) -> Option<String> {
    meaningful_string(raw)
}

fn meaningful_string(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("undefined")
        || trimmed.eq_ignore_ascii_case("null")
    {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub(crate) fn optional_path_field_keys(payload: &Value, keys: &[&str]) -> Option<String> {
    string_field_keys(payload, keys)
}

pub(crate) fn bool_field_keys(payload: &Value, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .find_map(|key| payload.get(key))
        .and_then(Value::as_bool)
}

pub(crate) fn usize_field_keys(payload: &Value, keys: &[&str]) -> Option<usize> {
    keys.iter()
        .find_map(|key| payload.get(key))
        .and_then(Value::as_u64)
        .map(|value| value as usize)
}
