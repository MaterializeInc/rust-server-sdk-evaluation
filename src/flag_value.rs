use log::warn;
use serde::{Deserialize, Serialize};

use crate::util::f64_to_i64_safe;

/// FlagValue represents any of the data types supported by JSON, all of which can be used for a
/// LaunchDarkly feature flag variation or a custom context attribute.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum FlagValue {
    /// Used when the value is a boolean.
    Bool(bool),
    /// Used when the value is a string.
    Str(String),
    /// Used when the value is a number.
    Number(f64),
    /// Used when the value is an arbitrary JSON value.
    Json(serde_json::Value),
}

impl From<bool> for FlagValue {
    fn from(b: bool) -> FlagValue {
        FlagValue::Bool(b)
    }
}

impl From<String> for FlagValue {
    fn from(s: String) -> FlagValue {
        FlagValue::Str(s)
    }
}

impl From<f64> for FlagValue {
    fn from(f: f64) -> FlagValue {
        FlagValue::Number(f)
    }
}

impl From<i64> for FlagValue {
    fn from(i: i64) -> FlagValue {
        FlagValue::Number(i as f64)
    }
}

impl From<serde_json::Value> for FlagValue {
    fn from(v: serde_json::Value) -> Self {
        use serde_json::Value;
        match v {
            Value::Bool(b) => b.into(),
            Value::Number(n) => {
                if let Some(f) = n.as_f64() {
                    f.into()
                } else {
                    warn!("unrepresentable number {}, converting to string", n);
                    FlagValue::Json(format!("{}", n).into())
                }
            }
            Value::String(s) => s.into(),
            Value::Null | Value::Object(_) | Value::Array(_) => FlagValue::Json(v),
        }
    }
}

impl FlagValue {
    /// Attempts to convert the FlagValue into a boolean representation, returning None if the
    /// conversion is invalid.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            FlagValue::Bool(b) => Some(*b),
            _ => {
                warn!("variation type is not bool but {:?}", self);
                None
            }
        }
    }

    /// Attempts to convert the FlagValue into a string representation, returning None if the
    /// conversion is invalid.
    pub fn as_string(&self) -> Option<String> {
        match self {
            FlagValue::Str(s) => Some(s.clone()),
            _ => {
                warn!("variation type is not str but {:?}", self);
                None
            }
        }
    }

    /// Attempts to convert the FlagValue into a float representation, returning None if the
    /// conversion is invalid.
    pub fn as_float(&self) -> Option<f64> {
        match self {
            FlagValue::Number(f) => Some(*f),
            _ => {
                warn!("variation type is not number but {:?}", self);
                None
            }
        }
    }

    /// Attempts to convert the FlagValue into a integer representation, returning None if the
    /// conversion is invalid.
    pub fn as_int(&self) -> Option<i64> {
        match self {
            FlagValue::Number(f) => f64_to_i64_safe(*f),
            _ => {
                warn!("variation type is not number but {:?}", self);
                None
            }
        }
    }

    /// Attempts to convert the FlagValue into an arbitrary JSON representation, returning None if the
    /// conversion is invalid.
    pub fn as_json(&self) -> Option<serde_json::Value> {
        use serde_json::Value;
        match self {
            FlagValue::Bool(b) => Some(Value::from(*b)),
            FlagValue::Str(s) => Some(Value::from(s.as_str())),
            FlagValue::Number(f) => Some(Value::from(*f)),
            FlagValue::Json(v) => Some(v.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use spectral::prelude::*;

    #[test]
    fn float_bounds() {
        let test_cases = vec![
            (1.99, Some(1)),
            (9007199254740990.0, Some(9007199254740990)),
            (9007199254740991.0, Some(9007199254740991)),
            (9007199254740992.0, None),
            (-1.99, Some(-1)),
            (-9007199254740990.0, Some(-9007199254740990)),
            (-9007199254740991.0, Some(-9007199254740991)),
            (-9007199254740992.0, None),
        ];
        for (have, expect) in test_cases {
            assert_that!(FlagValue::Number(have).as_int()).is_equal_to(expect);
        }
    }

    #[test]
    fn deserialization() {
        fn test_case(json: &str, expected: FlagValue) {
            assert_eq!(serde_json::from_str::<FlagValue>(json).unwrap(), expected);
        }

        test_case("1.0", FlagValue::Number(1.0));
        test_case("1", FlagValue::Number(1.0));
        test_case("true", FlagValue::Bool(true));
        test_case("\"foo\"", FlagValue::Str("foo".to_string()));
        test_case("{}", FlagValue::Json(json!({})));
    }

    #[test]
    fn can_handle_converting_between_types() {
        let value: FlagValue = true.into();
        assert_eq!(Some(true), value.as_bool());
        assert!(value.as_string().is_none());
        assert!(value.as_float().is_none());
        assert!(value.as_float().is_none());
        assert!(value.as_int().is_none());

        let value: FlagValue = String::from("testing").into();
        assert!(value.as_bool().is_none());
        assert_eq!(Some(String::from("testing")), value.as_string());
        assert!(value.as_float().is_none());
        assert!(value.as_float().is_none());
        assert!(value.as_int().is_none());

        let value: FlagValue = 1_f64.into();
        assert!(value.as_bool().is_none());
        assert!(value.as_string().is_none());
        assert_eq!(Some(1_f64), value.as_float());
        assert_eq!(Some(1_i64), value.as_int());

        let value: FlagValue = 1_i64.into();
        assert!(value.as_bool().is_none());
        assert!(value.as_string().is_none());
        assert_eq!(Some(1_f64), value.as_float());
        assert_eq!(Some(1_i64), value.as_int());

        let value: FlagValue = serde_json::Value::Bool(true).into();
        assert_eq!(Some(true), value.as_bool());
        assert_eq!(Some(serde_json::Value::Bool(true)), value.as_json());

        let value: FlagValue = serde_json::Value::String("testing".to_string()).into();
        assert_eq!(Some(String::from("testing")), value.as_string());
        assert_eq!(
            Some(serde_json::Value::String("testing".to_string())),
            value.as_json()
        );

        let value: FlagValue = json!(1_f64).into();
        assert_eq!(Some(1_f64), value.as_float());
        assert_eq!(Some(json!(1_f64)), value.as_json());

        let value: FlagValue = serde_json::Value::Array(vec![serde_json::Value::Bool(true)]).into();
        assert_eq!(
            Some(serde_json::Value::Array(vec![serde_json::Value::Bool(
                true
            )])),
            value.as_json()
        );
    }
}
