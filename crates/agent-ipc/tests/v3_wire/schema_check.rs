// Frozen schema subset checker from 8d6201dce:crates/mcp-server/src/tests.rs.
use serde_json::{Value, json};
pub fn accepts_schema(schema: &Value, value: &Value) -> bool {
    accepts_schema_root(schema, value, schema)
}
fn accepts_schema_root(schema: &Value, value: &Value, root: &Value) -> bool {
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let target = root
            .pointer(reference.strip_prefix('#').expect("local schema reference"))
            .expect("schema reference resolves");
        assert_eq!(
            schema.as_object().unwrap().len(),
            1,
            "reference-only schemas"
        );
        return accepts_schema_root(target, value, root);
    }
    if schema
        .get("const")
        .is_some_and(|expected| expected != value)
    {
        return false;
    }
    if schema
        .get("enum")
        .and_then(Value::as_array)
        .is_some_and(|values| !values.contains(value))
    {
        return false;
    }
    for (keyword, exactly_one) in [("anyOf", false), ("oneOf", true)] {
        if let Some(choices) = schema.get(keyword).and_then(Value::as_array) {
            let matches = choices
                .iter()
                .filter(|choice| accepts_schema_root(choice, value, root))
                .count();
            if matches == 0 || exactly_one && matches != 1 {
                return false;
            }
        }
    }
    let matches_type = |kind: &str| match kind {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        _ => false,
    };
    if let Some(kind) = schema.get("type") {
        if !match kind {
            Value::String(kind) => matches_type(kind),
            Value::Array(kinds) => kinds
                .iter()
                .any(|kind| kind.as_str().is_some_and(matches_type)),
            _ => false,
        } {
            return false;
        }
    }
    if let Some(object) = value.as_object() {
        if schema
            .get("required")
            .and_then(Value::as_array)
            .is_some_and(|keys| {
                keys.iter()
                    .any(|key| !object.contains_key(key.as_str().unwrap()))
            })
        {
            return false;
        }
        for (key, value) in object {
            if let Some(property) = schema.get("properties").and_then(|p| p.get(key)) {
                if !accepts_schema_root(property, value, root) {
                    return false;
                }
            } else if schema.get("additionalProperties") == Some(&json!(false)) {
                return false;
            }
        }
    }
    if let Some(array) = value.as_array() {
        if schema.get("uniqueItems") == Some(&json!(true))
            && array
                .iter()
                .enumerate()
                .any(|(index, value)| array[..index].contains(value))
        {
            return false;
        }
        if schema
            .get("minItems")
            .and_then(Value::as_u64)
            .is_some_and(|min| array.len() < (min as usize))
            || schema
                .get("maxItems")
                .and_then(Value::as_u64)
                .is_some_and(|max| array.len() > max as usize)
        {
            return false;
        }
        if schema.get("items").is_some_and(|item| {
            array
                .iter()
                .any(|value| !accepts_schema_root(item, value, root))
        }) {
            return false;
        }
    }
    if let Some(text) = value.as_str() {
        let length = text.chars().count();
        if schema
            .get("minLength")
            .and_then(Value::as_u64)
            .is_some_and(|min| length < min as usize)
            || schema
                .get("maxLength")
                .and_then(Value::as_u64)
                .is_some_and(|max| length > max as usize)
        {
            return false;
        }
        if schema.get("pattern") == Some(&json!("^[0-9a-f]{64}$"))
            && (text.len() != 64
                || !text
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        {
            return false;
        }
    }
    if let Some(number) = value.as_f64() {
        if schema
            .get("minimum")
            .and_then(Value::as_f64)
            .is_some_and(|min| number < min)
            || schema
                .get("maximum")
                .and_then(Value::as_f64)
                .is_some_and(|max| number > max)
        {
            return false;
        }
    }
    true
}
