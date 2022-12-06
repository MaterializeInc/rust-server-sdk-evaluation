use std::collections::HashMap;

use chrono::{self, LocalResult, TimeZone, Utc};

use lazy_static::lazy_static;
use log::warn;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::util::f64_to_i64_safe;

lazy_static! {
    static ref VERSION_NUMERIC_COMPONENTS_REGEX: Regex =
        Regex::new(r"^\d+(\.\d+)?(\.\d+)?").unwrap();
}

/// An attribute value represents possible values that can be stored in a [crate::Context].
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub enum AttributeValue {
    /// Stores a string value.
    String(String),
    /// Stores an array of attribute values.
    Array(Vec<AttributeValue>),
    /// Stores a number.
    Number(f64),
    /// Stores a boolean.
    Bool(bool),
    /// Stores a map of attribute values.
    Object(HashMap<String, AttributeValue>),
    /// Stores a null value.
    Null,
}

impl From<&str> for AttributeValue {
    fn from(s: &str) -> AttributeValue {
        AttributeValue::String(s.to_owned())
    }
}

impl From<String> for AttributeValue {
    fn from(s: String) -> AttributeValue {
        AttributeValue::String(s)
    }
}

impl From<bool> for AttributeValue {
    fn from(b: bool) -> AttributeValue {
        AttributeValue::Bool(b)
    }
}

impl From<i64> for AttributeValue {
    fn from(i: i64) -> Self {
        AttributeValue::Number(i as f64)
    }
}

impl From<f64> for AttributeValue {
    fn from(f: f64) -> Self {
        AttributeValue::Number(f)
    }
}

impl<T> From<Vec<T>> for AttributeValue
where
    AttributeValue: From<T>,
{
    fn from(v: Vec<T>) -> AttributeValue {
        v.into_iter().collect()
    }
}

impl<S, T> From<HashMap<S, T>> for AttributeValue
where
    String: From<S>,
    AttributeValue: From<T>,
{
    fn from(hashmap: HashMap<S, T>) -> AttributeValue {
        hashmap.into_iter().collect()
    }
}

impl<T> FromIterator<T> for AttributeValue
where
    AttributeValue: From<T>,
{
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        AttributeValue::Array(iter.into_iter().map(AttributeValue::from).collect())
    }
}

impl<S, T> FromIterator<(S, T)> for AttributeValue
where
    String: From<S>,
    AttributeValue: From<T>,
{
    fn from_iter<I: IntoIterator<Item = (S, T)>>(iter: I) -> Self {
        AttributeValue::Object(
            iter.into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        )
    }
}

impl From<&Value> for AttributeValue {
    fn from(v: &Value) -> Self {
        match v {
            Value::Null => AttributeValue::Null,
            Value::Bool(b) => AttributeValue::Bool(*b),
            Value::Number(n) => match n.as_f64() {
                Some(float) => AttributeValue::Number(float),
                None => {
                    warn!("could not interpret '{:?}' as f64", n);
                    AttributeValue::String(n.to_string())
                }
            },
            Value::String(str) => AttributeValue::String(str.clone()),
            Value::Array(arr) => {
                AttributeValue::Array(arr.iter().map(AttributeValue::from).collect())
            }
            Value::Object(obj) => {
                AttributeValue::Object(obj.iter().map(|(k, v)| (k.into(), v.into())).collect())
            }
        }
    }
}

impl AttributeValue {
    /// Returns None unless self is a String. It will not convert.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            AttributeValue::String(s) => Some(s),
            _ => None,
        }
    }

    /// Returns the wrapped value as a float for numeric types, and None otherwise.
    pub fn to_f64(&self) -> Option<f64> {
        match self {
            AttributeValue::Number(f) => Some(*f),
            _ => None,
        }
    }

    /// Returns None unless self is a bool. It will not convert.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            AttributeValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Attempt to convert any of the following into a chrono::DateTime in UTC:
    ///  * RFC3339/ISO8601 timestamp (example: "2016-04-16T17:09:12.759-07:00")
    ///  * Unix epoch milliseconds as number
    /// It will return None if the conversion fails or if no conversion is possible.
    pub fn to_datetime(&self) -> Option<chrono::DateTime<Utc>> {
        match self {
            AttributeValue::Number(millis) => {
                f64_to_i64_safe(*millis).and_then(|millis| match Utc.timestamp_millis_opt(millis) {
                    LocalResult::None | LocalResult::Ambiguous(_, _) => None,
                    LocalResult::Single(time) => Some(time),
                })
            }
            AttributeValue::String(s) => chrono::DateTime::parse_from_rfc3339(s)
                .map(|dt| dt.with_timezone(&Utc))
                .ok(),
            AttributeValue::Bool(_) | AttributeValue::Null => None,
            other => {
                warn!(
                    "Don't know how or whether to convert attribute value {:?} to datetime",
                    other
                );
                None
            }
        }
    }

    /// Attempt to parse a string attribute into a semver version.
    ///
    /// It will return None if it cannot parse it, or for non-string attributes.
    pub fn as_semver(&self) -> Option<semver::Version> {
        let version_str = self.as_str()?;
        semver::Version::parse(version_str)
            .ok()
            .or_else(|| AttributeValue::parse_semver_loose(version_str))
            .map(|mut version| {
                version.build = semver::BuildMetadata::EMPTY;
                version
            })
    }

    fn parse_semver_loose(version_str: &str) -> Option<semver::Version> {
        let parts = VERSION_NUMERIC_COMPONENTS_REGEX.captures(version_str)?;

        let numeric_parts = parts.get(0).unwrap();
        let mut transformed_version_str = numeric_parts.as_str().to_string();

        for i in 1..parts.len() {
            if parts.get(i).is_none() {
                transformed_version_str.push_str(".0");
            }
        }

        let rest = &version_str[numeric_parts.end()..];
        transformed_version_str.push_str(rest);

        semver::Version::parse(&transformed_version_str).ok()
    }

    /// Find the AttributeValue based off the provided predicate `p`.
    pub fn find<P>(&self, p: P) -> Option<&AttributeValue>
    where
        P: Fn(&AttributeValue) -> bool,
    {
        match self {
            AttributeValue::String(_)
            | AttributeValue::Number(_)
            | AttributeValue::Bool(_)
            | AttributeValue::Object(_) => {
                if p(self) {
                    Some(self)
                } else {
                    None
                }
            }
            AttributeValue::Array(values) => values.iter().find(|v| p(v)),
            AttributeValue::Null => None,
        }
    }

    #[allow(clippy::float_cmp)]
    pub(crate) fn as_bucketable(&self) -> Option<String> {
        match self {
            AttributeValue::String(s) => Some(s.clone()),
            AttributeValue::Number(f) => {
                // We only support integer values as bucketable
                f64_to_i64_safe(*f).and_then(|i| {
                    if i as f64 == *f {
                        Some(i.to_string())
                    } else {
                        None
                    }
                })
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AttributeValue;
    use maplit::hashmap;

    #[test]
    fn collect_array() {
        assert_eq!(
            Some(10_i64).into_iter().collect::<AttributeValue>(),
            AttributeValue::Array(vec![AttributeValue::Number(10_f64)])
        );
    }

    #[test]
    fn collect_object() {
        assert_eq!(
            Some(("abc", 10_i64))
                .into_iter()
                .collect::<AttributeValue>(),
            AttributeValue::Object(hashmap! {"abc".to_string() => AttributeValue::Number(10_f64)})
        );
    }

    #[test]
    fn deserialization() {
        fn test_case(json: &str, expected: AttributeValue) {
            assert_eq!(
                serde_json::from_str::<AttributeValue>(json).unwrap(),
                expected
            );
        }

        test_case("1.0", AttributeValue::Number(1.0));
        test_case("1", AttributeValue::Number(1.0));
        test_case("true", AttributeValue::Bool(true));
        test_case("\"foo\"", AttributeValue::String("foo".to_string()));
        test_case("{}", AttributeValue::Object(hashmap![]));
        test_case(
            r#"{"foo":123}"#,
            AttributeValue::Object(hashmap!["foo".to_string() => AttributeValue::Number(123.0)]),
        );
    }
}
