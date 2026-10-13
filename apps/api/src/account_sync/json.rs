//! Strict JSON for sync v1 documents: duplicate object keys are refused before
//! any schema check, and every object is closed (exact keys).
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};
use std::fmt;

pub const MAX_SAFE: u64 = 9_007_199_254_740_991;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invalid(pub &'static str);
pub type Checked<T> = Result<T, Invalid>;

/// Parses UTF-8 JSON, refusing duplicate keys at any depth. Malformed UTF-8
/// and unpaired surrogate escapes are already refused by `serde_json`.
pub fn parse(bytes: &[u8]) -> Checked<Value> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = Strict
        .deserialize(&mut deserializer)
        .map_err(|_| Invalid("malformed_json"))?;
    deserializer.end().map_err(|_| Invalid("malformed_json"))?;
    Ok(value)
}

struct Strict;
impl<'de> DeserializeSeed<'de> for Strict {
    type Value = Value;
    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<Value, D::Error> {
        d.deserialize_any(StrictVisitor)
    }
}
struct StrictVisitor;
impl<'de> Visitor<'de> for StrictVisitor {
    type Value = Value;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("JSON")
    }
    fn visit_bool<E>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }
    fn visit_i64<E>(self, v: i64) -> Result<Value, E> {
        Ok(Value::from(v))
    }
    fn visit_u64<E>(self, v: u64) -> Result<Value, E> {
        Ok(Value::from(v))
    }
    fn visit_f64<E>(self, v: f64) -> Result<Value, E> {
        Ok(serde_json::Number::from_f64(v).map_or(Value::Null, Value::Number))
    }
    fn visit_str<E>(self, v: &str) -> Result<Value, E> {
        Ok(Value::String(v.to_owned()))
    }
    fn visit_string<E>(self, v: String) -> Result<Value, E> {
        Ok(Value::String(v))
    }
    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_none<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut out = Vec::new();
        while let Some(item) = seq.next_element_seed(Strict)? {
            out.push(item);
        }
        Ok(Value::Array(out))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut out = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            let value = map.next_value_seed(Strict)?;
            if out.insert(key, value).is_some() {
                return Err(de::Error::custom("duplicate key"));
            }
        }
        Ok(Value::Object(out))
    }
}

/// The object's fields, which must be exactly `keys`.
pub fn object<'a>(value: &'a Value, keys: &[&str]) -> Checked<&'a Map<String, Value>> {
    let map = value.as_object().ok_or(Invalid("not_an_object"))?;
    if map.len() != keys.len() || !keys.iter().all(|k| map.contains_key(*k)) {
        return Err(Invalid("unexpected_fields"));
    }
    Ok(map)
}
pub fn string<'a>(map: &'a Map<String, Value>, key: &str) -> Checked<&'a str> {
    map[key].as_str().ok_or(Invalid("not_a_string"))
}
pub fn boolean(map: &Map<String, Value>, key: &str) -> Checked<bool> {
    map[key].as_bool().ok_or(Invalid("not_a_boolean"))
}
pub fn uint(map: &Map<String, Value>, key: &str) -> Checked<u64> {
    map[key]
        .as_u64()
        .filter(|n| *n <= MAX_SAFE)
        .ok_or(Invalid("not_a_safe_integer"))
}
pub fn positive(map: &Map<String, Value>, key: &str) -> Checked<u64> {
    uint(map, key).and_then(|n| {
        if n >= 1 {
            Ok(n)
        } else {
            Err(Invalid("not_positive"))
        }
    })
}
pub fn literal(map: &Map<String, Value>, key: &str, expected: &str) -> Checked<()> {
    (string(map, key)? == expected)
        .then_some(())
        .ok_or(Invalid("unexpected_value"))
}
/// A lowercase hyphenated UUID, exactly as the contract's pattern requires.
pub fn uuid(map: &Map<String, Value>, key: &str) -> Checked<uuid::Uuid> {
    parse_uuid(string(map, key)?)
}
pub fn parse_uuid(text: &str) -> Checked<uuid::Uuid> {
    let valid = text.len() == 36
        && text.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_digit() || (b'a'..=b'f').contains(&b),
        });
    valid
        .then(|| uuid::Uuid::parse_str(text).ok())
        .flatten()
        .ok_or(Invalid("invalid_uuid"))
}
