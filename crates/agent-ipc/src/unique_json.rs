//! Reject duplicate object names before a Value/map can discard them.
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Value};

struct UniqueValue(Value);

impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct UniqueVisitor;
        impl<'de> Visitor<'de> for UniqueVisitor {
            type Value = UniqueValue;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("JSON without duplicate object names")
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Null))
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Self::Value, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
                serde_json::Number::from_f64(v)
                    .map(|v| UniqueValue(Value::Number(v)))
                    .ok_or_else(|| E::custom("invalid JSON number"))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut out = Vec::new();
                while let Some(UniqueValue(v)) = seq.next_element()? {
                    out.push(v);
                }
                Ok(UniqueValue(Value::Array(out)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut out = Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if out.contains_key(&key) {
                        return Err(de::Error::custom(format!("duplicate JSON name: {key}")));
                    }
                    let UniqueValue(value) = map.next_value()?;
                    out.insert(key, value);
                }
                Ok(UniqueValue(Value::Object(out)))
            }
        }
        deserializer.deserialize_any(UniqueVisitor)
    }
}

pub fn parse_unique_json(bytes: &[u8]) -> serde_json::Result<Value> {
    serde_json::from_slice::<UniqueValue>(bytes).map(|v| v.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn operations_duplicate_names_are_rejected_before_map_projection() {
        for bytes in [br#"{"keys":{"FsCopyImage":[],"FsCopyImage":null}}"#.as_slice(),
            br#"{"inputs":{"gesture.image":[{"pattern":"DR","action":"none","action":"copy_files"}]}}"#] {
            assert!(parse_unique_json(bytes).is_err());
        }
        assert!(parse_unique_json(br#"{"keys":{"FsCopyImage":["F13","F13"]}}"#).is_ok());
    }
}
