//! JSON.stringify-compatible bytes for captured identities and hashes.
//! Ordinary object keys keep insertion order; array-index keys precede them.
use serde_json::Value;
fn index(key: &str) -> Option<u32> {
    let value: u32 = key.parse().ok()?;
    (value != u32::MAX && value.to_string() == key).then_some(value)
}
fn append(value: &Value, output: &mut String) {
    match value {
        Value::Null => output.push_str("null"),
        Value::Bool(v) => output.push_str(if *v { "true" } else { "false" }),
        Value::Number(n) => {
            output.push_str(ryu_js::Buffer::new().format(n.as_f64().expect("finite JSON number")))
        }
        Value::String(s) => output.push_str(&serde_json::to_string(s).expect("JSON string")),
        Value::Array(values) => {
            output.push('[');
            for (i, value) in values.iter().enumerate() {
                if i != 0 {
                    output.push(',');
                }
                append(value, output);
            }
            output.push(']');
        }
        Value::Object(values) => {
            output.push('{');
            let mut indexed: Vec<_> = values
                .keys()
                .filter_map(|k| index(k).map(|i| (i, k)))
                .collect();
            indexed.sort_unstable_by_key(|(i, _)| *i);
            let keys = indexed
                .into_iter()
                .map(|(_, k)| k)
                .chain(values.keys().filter(|k| index(k).is_none()));
            for (i, key) in keys.enumerate() {
                if i != 0 {
                    output.push(',');
                }
                output.push_str(&serde_json::to_string(key).expect("JSON object key"));
                output.push(':');
                append(&values[key], output);
            }
            output.push('}');
        }
    }
}
/// Preserve the retained runtime's serialization contract for valid JSON values.
pub fn stringify(value: &Value) -> String {
    let mut output = String::new();
    append(value, &mut output);
    output
}
pub(crate) fn same(left: &Value, right: &Value) -> bool {
    stringify(left) == stringify(right)
}
