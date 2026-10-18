use super::*;
use serde_json::{Map, Value};
pub(super) fn object(value: &Value) -> Result<&Map<String, Value>> {
    value.as_object().ok_or_else(invalid)
}
pub(super) fn strict(value: &Value, keys: &[&str]) -> Result<()> {
    if object(value)?
        .keys()
        .any(|key| !keys.contains(&key.as_str()))
    {
        return Err(invalid());
    }
    Ok(())
}
pub(super) fn text(value: &Value, min: usize, max: usize, trim: bool) -> Result<String> {
    let s = value.as_str().ok_or_else(invalid)?;
    let s = if trim {
        crewroom_core::store::trim_text(s)
    } else {
        s
    };
    if !(min..=max).contains(&s.chars().count()) {
        return Err(invalid());
    }
    Ok(s.into())
}
pub(super) fn string(value: &Value) -> Result<String> {
    text(value, 0, usize::MAX, false)
}
pub(super) fn array(value: &Value) -> Result<&Vec<Value>> {
    value.as_array().ok_or_else(invalid)
}
pub(super) fn integer(value: &Value, min: i64) -> Result<i64> {
    value
        .as_f64()
        .filter(|n| {
            n.is_finite()
                && n.fract() == 0.0
                && *n >= min as f64
                && *n <= 9_007_199_254_740_991.0
                && *n >= -9_007_199_254_740_991.0
        })
        .map(|n| n as i64)
        .ok_or_else(invalid)
}
pub(super) fn boolean(value: &Value) -> Result<bool> {
    value.as_bool().ok_or_else(invalid)
}
pub(super) fn digits(value: &Value) -> Result<String> {
    let s = text(value, 1, usize::MAX, false)?;
    if !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    Ok(s)
}
pub(super) fn account(value: &Value) -> Result<String> {
    let s = text(value, 1, 200, false)?;
    if !s
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b":_-".contains(&b))
    {
        return Err(invalid());
    }
    Ok(s)
}
pub(super) fn component(value: &str) -> String {
    let mut result = String::new();
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            result.push(b as char);
        } else {
            use std::fmt::Write;
            write!(result, "%{b:02X}").unwrap();
        }
    }
    result
}
pub(super) fn date(value: &Value) -> Result<String> {
    let s = string(value)?;
    if crewroom_core::source_time::parse(&s).is_none() {
        return Err(invalid());
    }
    Ok(s)
}
pub(super) fn url_text(value: &Value) -> Result<String> {
    let s = string(value)?;
    url::Url::parse(&s).map_err(|_| invalid())?;
    Ok(s)
}
pub(super) fn hash(value: &Value, abbreviated: bool) -> Result<String> {
    let s = text(value, if abbreviated { 7 } else { 40 }, 64, false)?;
    if !s
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid());
    }
    Ok(s)
}
/// The one coercing provider field is Bitbucket's PR pipeline id.
pub(super) fn coerced_id(value: &Value) -> Result<i64> {
    fn array_text(value: &Value) -> String {
        match value {
            Value::Null => String::new(),
            Value::String(s) => s.clone(),
            Value::Array(a) => a.iter().map(array_text).collect::<Vec<_>>().join(","),
            Value::Object(_) => "[object Object]".into(),
            _ => crewroom_core::serialized::stringify(value),
        }
    }
    let number = match value {
        Value::Null => 0.0,
        Value::Bool(b) => {
            if *b {
                1.0
            } else {
                0.0
            }
        }
        Value::Number(n) => n.as_f64().ok_or_else(invalid)?,
        Value::Object(_) => return Err(invalid()),
        _ => {
            let raw = array_text(value);
            let s = crewroom_core::store::trim_text(&raw);
            if s.is_empty() {
                0.0
            } else if s.starts_with("0x")
                || s.starts_with("0X")
                || s.starts_with("0b")
                || s.starts_with("0B")
                || s.starts_with("0o")
                || s.starts_with("0O")
            {
                let radix = match s.as_bytes()[1] {
                    b'x' | b'X' => 16,
                    b'b' | b'B' => 2,
                    _ => 8,
                };
                if s.len() == 2 {
                    return Err(invalid());
                }
                s[2..].chars().try_fold(0.0, |n, c| {
                    c.to_digit(radix)
                        .map(|v| n * radix as f64 + v as f64)
                        .ok_or_else(invalid)
                })?
            } else {
                if !s
                    .bytes()
                    .all(|b| b.is_ascii_digit() || b"+-.eE".contains(&b))
                {
                    return Err(invalid());
                }
                s.parse::<f64>().map_err(|_| invalid())?
            }
        }
    };
    if !number.is_finite()
        || number.fract() != 0.0
        || !(1.0..=9_007_199_254_740_991.0).contains(&number)
    {
        return Err(invalid());
    }
    Ok(number as i64)
}
