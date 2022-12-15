use std::collections::{HashMap, HashSet};

use chrono::{self, TimeZone, Utc};
use lazy_static::lazy_static;
use log::warn;
use regex::Regex;
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha1::{Digest, Sha1};

use crate::util::f64_to_i64_safe;

const USER_CUSTOM_STARTING_CAPACITY: usize = 10;
const BUCKET_SCALE_INT: i64 = 0x0FFF_FFFF_FFFF_FFFF;
const BUCKET_SCALE: f32 = BUCKET_SCALE_INT as f32;

lazy_static! {
    static ref VERSION_NUMERIC_COMPONENTS_REGEX: Regex =
        Regex::new(r"^\d+(\.\d+)?(\.\d+)?").unwrap();
}

/// AttributeValue is an enum representing possible values that can be stored in a user attribute.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub enum AttributeValue {
    /// Used when storing a string value.
    String(String),
    /// Used when storing an array of AttributeValues.
    Array(Vec<AttributeValue>),
    /// Used when storing a number.
    Number(f64),
    /// Used when storing a boolean.
    Bool(bool),
    /// Used when storing a complex map of values.
    Object(HashMap<String, AttributeValue>),
    /// Used to represent a null value.
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

impl<T> std::iter::FromIterator<T> for AttributeValue
where
    AttributeValue: From<T>,
{
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        AttributeValue::Array(iter.into_iter().map(AttributeValue::from).collect())
    }
}

impl<S, T> std::iter::FromIterator<(S, T)> for AttributeValue
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
                f64_to_i64_safe(*millis).map(|millis| Utc.timestamp_millis(millis))
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
    fn as_bucketable(&self) -> Option<String> {
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

/// When sending [User] information to LaunchDarkly, we may want to redact select user properties.
///
/// This struct handles the serialization rules for this redaction process. See
/// <https://docs.launchdarkly.com/sdk/features/user-config#private-user-attributes-in-the-rust-sdk>
/// for more information.
pub struct UserAttributes<'dispatcher> {
    user: User,
    all_attributes_private: bool,
    global_private_attributes: &'dispatcher HashSet<String>,
}

impl<'dispatcher> UserAttributes<'dispatcher> {
    /// Construct a new [UserAttributes] struct with the given parameters.
    pub fn from_user(
        user: User,
        all_attributes_private: bool,
        private_attributes: &'dispatcher HashSet<String>,
    ) -> Self {
        Self {
            user,
            all_attributes_private,
            global_private_attributes: private_attributes,
        }
    }

    fn is_private_attribute(&self, attr: &str) -> bool {
        self.all_attributes_private
            || self.global_private_attributes.contains(attr)
            || self.user.is_private_attribute(attr)
    }
}

impl<'dispatcher> Serialize for UserAttributes<'dispatcher> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let fields: Vec<String> = vec![
            "key".into(),
            "secondary".into(),
            "ip".into(),
            "country".into(),
            "email".into(),
            "firstName".into(),
            "lastName".into(),
            "avatar".into(),
            "name".into(),
            "anonymous".into(),
        ];

        let mut private_attribute_names = Vec::new();
        let mut serialize_map = serializer.serialize_map(None)?;
        for field in fields {
            match self.user.value_of(&field) {
                None => (),
                Some(_) if field != "key" && self.is_private_attribute(&field) => {
                    private_attribute_names.push(field)
                }
                Some(attr_value) => serialize_map.serialize_entry(&field, &attr_value)?,
            }
        }

        let mut custom: HashMap<String, AttributeValue> = HashMap::new();
        for (key, value) in self.user.custom.clone().into_iter() {
            if self.is_private_attribute(&key) {
                private_attribute_names.push(key)
            } else {
                custom.insert(key, value);
            }
        }

        if !private_attribute_names.is_empty() {
            serialize_map.serialize_entry("privateAttrs", &private_attribute_names)?;
        }

        if !custom.is_empty() {
            serialize_map.serialize_entry("custom", &custom)?;
        }

        serialize_map.end()
    }
}

/// A User contains specific attributes of a user browsing your site. The only mandatory property is the Key,
/// which must uniquely identify each user. For authenticated users, this may be a username or e-mail address.
/// For anonymous users, this could be an IP address or session ID.
///
/// Besides the mandatory key, User supports two kinds of optional attributes: interpreted attributes (e.g.
/// IP and Country) and custom attributes.  LaunchDarkly can parse interpreted attributes and attach meaning
/// to them. For example, from an IP address, LaunchDarkly can do a geo IP lookup and determine the user's
/// country.
///
/// Custom attributes are not parsed by LaunchDarkly. They can be used in custom rules-- for example, a custom
/// attribute such as "customer_ranking" can be used to launch a feature to the top 10% of users on a site.
///
/// User fields are immutable and can be accessed only via getter methods. To construct a User, use
/// the [UserBuilder] by calling the [User::with_key].
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct User {
    #[serde(rename = "key")]
    _key: String,
    #[serde(rename = "secondary", skip_serializing_if = "Option::is_none")]
    _secondary: Option<String>,
    #[serde(rename = "ip", skip_serializing_if = "Option::is_none")]
    _ip: Option<String>,
    #[serde(rename = "country", skip_serializing_if = "Option::is_none")]
    _country: Option<String>,
    #[serde(rename = "email", skip_serializing_if = "Option::is_none")]
    _email: Option<String>,
    #[serde(rename = "firstName", skip_serializing_if = "Option::is_none")]
    _first_name: Option<String>,
    #[serde(rename = "lastName", skip_serializing_if = "Option::is_none")]
    _last_name: Option<String>,
    #[serde(rename = "avatar", skip_serializing_if = "Option::is_none")]
    _avatar: Option<String>,
    #[serde(rename = "name", skip_serializing_if = "Option::is_none")]
    _name: Option<String>,
    #[serde(rename = "anonymous", skip_serializing_if = "Option::is_none")]
    _anonymous: Option<bool>,

    #[serde(default, deserialize_with = "deserialize_null_default")]
    custom: HashMap<String, AttributeValue>,

    #[serde(rename = "privateAttributeNames", skip_serializing, default)]
    private_attributes: HashSet<String>,
}

fn deserialize_null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    T: Default + Deserialize<'de>,
    D: Deserializer<'de>,
{
    let opt = Option::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default())
}

/// An error type used user attribute type failures.
#[derive(Debug)]
pub struct TypeError {
    key: &'static str,
    expected_type: &'static str,
    actual_type: &'static str,
}

impl TypeError {
    fn new(key: &'static str, expected_type: &'static str, actual_value: &AttributeValue) -> Self {
        TypeError {
            key,
            expected_type,
            actual_type: match actual_value {
                AttributeValue::Array(_) => "Array",
                AttributeValue::Bool(_) => "Bool",
                AttributeValue::Number(_) => "Number",
                AttributeValue::Null => "Null",
                AttributeValue::Object(_) => "Object",
                AttributeValue::String(_) => "String",
            },
        }
    }
}

impl std::fmt::Display for TypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Attribute {} must be {}, not {})",
            self.key, self.expected_type, self.actual_type
        )
    }
}

impl std::error::Error for TypeError {}

#[derive(Clone, Copy)]
pub(crate) enum BucketPrefix<'a> {
    KeyAndSalt(&'a str, &'a str),
    Seed(i64),
}

impl<'a> BucketPrefix<'a> {
    fn write_hash(&self, hash: &mut Sha1) {
        match self {
            BucketPrefix::KeyAndSalt(key, salt) => {
                hash.update(key.as_bytes());
                hash.update(b".");
                hash.update(salt.as_bytes());
            }
            BucketPrefix::Seed(seed) => {
                let seed_str = seed.to_string();
                hash.update(seed_str.as_bytes());
            }
        }
    }
}

impl User {
    /// Create a new [UserBuilder], seeding it with the provided user key.
    pub fn with_key(key: impl Into<String>) -> UserBuilder {
        UserBuilder::new(key)
    }

    /// Returns the key of the user.
    pub fn key(&self) -> &str {
        &self._key
    }

    /// Returns the secondary key of the user, if any.
    ///
    /// This affects feature flag targeting
    /// (<https://docs.launchdarkly.com/docs/targeting-users#section-targeting-rules-based-on-user-attributes>)
    /// as follows: if you have chosen to bucket users by a specific attribute, the secondary key
    /// (if set) is used to further distinguish between users who are otherwise identical according
    /// to that attribute.
    pub fn secondary(&self) -> Option<&str> {
        self._secondary.as_deref()
    }

    /// Returns the ip of the user, if any.
    pub fn ip(&self) -> Option<&str> {
        self._ip.as_deref()
    }

    /// Returns the country of the user, if any.
    pub fn country(&self) -> Option<&str> {
        self._country.as_deref()
    }

    /// Returns the email of the user, if any.
    pub fn email(&self) -> Option<&str> {
        self._email.as_deref()
    }

    /// Returns the first name of the user, if any.
    pub fn first_name(&self) -> Option<&str> {
        self._first_name.as_deref()
    }

    /// Returns the last name of the user, if any.
    pub fn last_name(&self) -> Option<&str> {
        self._last_name.as_deref()
    }

    /// Returns the avatar of the user, if any.
    pub fn avatar(&self) -> Option<&str> {
        self._avatar.as_deref()
    }

    /// Returns the name of the user, if any.
    pub fn name(&self) -> Option<&str> {
        self._name.as_deref()
    }

    /// Returns the anonymous attribute of the user.
    ///
    /// If a user is anonymous, the user key will not appear on your LaunchDarkly dashboard.
    pub fn anonymous(&self) -> Option<bool> {
        self._anonymous
    }

    /// Return the value of the attribute named `attr`.
    pub fn value_of(&self, attr: &str) -> Option<AttributeValue> {
        match attr {
            "key" => Some(AttributeValue::String(self._key.clone())),
            "secondary" => self._secondary.as_deref().map(AttributeValue::from),
            "ip" => self._ip.as_deref().map(AttributeValue::from),
            "country" => self._country.as_deref().map(AttributeValue::from),
            "email" => self._email.as_deref().map(AttributeValue::from),
            "firstName" => self._first_name.as_deref().map(AttributeValue::from),
            "lastName" => self._last_name.as_deref().map(AttributeValue::from),
            "avatar" => self._avatar.as_deref().map(AttributeValue::from),
            "name" => self._name.as_deref().map(AttributeValue::from),
            "anonymous" => self._anonymous.map(AttributeValue::from),
            _ => self.custom.get(attr).cloned(),
        }
    }

    fn is_private_attribute(&self, attr: &str) -> bool {
        self.private_attributes.contains(attr)
    }

    /// Set the attributed named `key` to the value `value`.
    ///
    /// If there is a type conversion issue when setting the attribute, return an error of type
    /// [TypeError].
    pub fn attribute<T: Into<AttributeValue>>(
        &mut self,
        key: &str,
        value: T,
    ) -> Result<(), TypeError> {
        let value: AttributeValue = value.into();
        match key {
            "key" => {
                self._key = value
                    .as_str()
                    .ok_or_else(|| TypeError::new("key", "String", &value))?
                    .to_string()
            }
            "secondary" => {
                self._secondary = Some(
                    value
                        .as_str()
                        .ok_or_else(|| TypeError::new("secondary", "String", &value))?
                        .to_string(),
                )
            }
            "ip" => {
                self._ip = Some(
                    value
                        .as_str()
                        .ok_or_else(|| TypeError::new("ip", "String", &value))?
                        .to_string(),
                )
            }
            "country" => {
                self._country = Some(
                    value
                        .as_str()
                        .ok_or_else(|| TypeError::new("country", "String", &value))?
                        .to_string(),
                )
            }
            "email" => {
                self._email = Some(
                    value
                        .as_str()
                        .ok_or_else(|| TypeError::new("email", "String", &value))?
                        .to_string(),
                )
            }
            "firstName" => {
                self._first_name = Some(
                    value
                        .as_str()
                        .ok_or_else(|| TypeError::new("firstName", "String", &value))?
                        .to_string(),
                )
            }
            "lastName" => {
                self._last_name = Some(
                    value
                        .as_str()
                        .ok_or_else(|| TypeError::new("lastName", "String", &value))?
                        .to_string(),
                )
            }
            "avatar" => {
                self._avatar = Some(
                    value
                        .as_str()
                        .ok_or_else(|| TypeError::new("avatar", "String", &value))?
                        .to_string(),
                )
            }
            "name" => {
                self._name = Some(
                    value
                        .as_str()
                        .ok_or_else(|| TypeError::new("name", "String", &value))?
                        .to_string(),
                )
            }
            "anonymous" => {
                self._anonymous = Some(
                    value
                        .as_bool()
                        .ok_or_else(|| TypeError::new("anonymous", "Bool", &value))?,
                )
            }
            _ => {
                let _ = self.custom.insert(key.to_string(), value);
            }
        }
        Ok(())
    }

    pub(crate) fn bucket(&self, by_attr: Option<&str>, prefix: BucketPrefix) -> f32 {
        let attr_value = match by_attr {
            Some(attr) => self.value_of(attr),
            None => Some(AttributeValue::String(self._key.clone())),
        };
        self._bucket(attr_value.as_ref(), prefix).unwrap_or(0.0)
    }

    fn _bucket(&self, attr_value: Option<&AttributeValue>, prefix: BucketPrefix) -> Option<f32> {
        let mut id = attr_value?.as_bucketable()?;

        if let Some(secondary) = self.secondary() {
            id.push('.');
            id.push_str(secondary);
        }

        let mut hash = Sha1::new();
        prefix.write_hash(&mut hash);
        hash.update(b".");
        hash.update(id.as_bytes());

        let digest = hash.finalize();
        let hexhash = base16ct::lower::encode_string(&digest);

        let hexhash_15 = &hexhash[..15]; // yes, 15 chars, not 16
        let numhash = i64::from_str_radix(hexhash_15, 16).unwrap();

        Some(numhash as f32 / BUCKET_SCALE)
    }
}

/// Contains methods for configuring a user.
pub struct UserBuilder {
    key: String,
    secondary: Option<String>,
    ip: Option<String>,
    country: Option<String>,
    email: Option<String>,
    first_name: Option<String>,
    last_name: Option<String>,
    avatar: Option<String>,
    name: Option<String>,
    anonymous: Option<bool>,
    custom: HashMap<String, AttributeValue>,
    private_attributes: HashSet<String>,
}

impl UserBuilder {
    /// Create a new user builder, setting the user key value to `key`.
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            secondary: None,
            ip: None,
            country: None,
            email: None,
            first_name: None,
            last_name: None,
            avatar: None,
            name: None,
            anonymous: None,
            custom: HashMap::with_capacity(USER_CUSTOM_STARTING_CAPACITY),
            // The same capacity as the custom container + the 9 built-in fields we might hide
            private_attributes: HashSet::with_capacity(USER_CUSTOM_STARTING_CAPACITY + 9),
        }
    }

    /// Set the secondary attribute for this builder instance.
    pub fn secondary(&mut self, secondary: impl Into<String>) -> &mut Self {
        self.secondary = Some(secondary.into());
        self
    }

    /// Set the ip attribute for this builder instance.
    pub fn ip(&mut self, ip: impl Into<String>) -> &mut Self {
        self.ip = Some(ip.into());
        self
    }

    /// Set the country attribute for this builder instance.
    pub fn country(&mut self, country: impl Into<String>) -> &mut Self {
        self.country = Some(country.into());
        self
    }

    /// Set the email attribute for this builder instance.
    pub fn email(&mut self, email: impl Into<String>) -> &mut Self {
        self.email = Some(email.into());
        self
    }

    /// Set the first name attribute for this builder instance.
    pub fn first_name(&mut self, first_name: impl Into<String>) -> &mut Self {
        self.first_name = Some(first_name.into());
        self
    }

    /// Set the last name attribute for this builder instance.
    pub fn last_name(&mut self, last_name: impl Into<String>) -> &mut Self {
        self.last_name = Some(last_name.into());
        self
    }

    /// Set the avatar attribute for this builder instance.
    pub fn avatar(&mut self, avatar: impl Into<String>) -> &mut Self {
        self.avatar = Some(avatar.into());
        self
    }

    /// Set the name attribute for this builder instance.
    pub fn name(&mut self, name: impl Into<String>) -> &mut Self {
        self.name = Some(name.into());
        self
    }

    /// Set the anonymous attribute for this builder instance.
    pub fn anonymous(&mut self, anonymous: bool) -> &mut Self {
        self.anonymous = Some(anonymous);
        self
    }

    /// Set the custom attributes for this builder instance.
    pub fn custom(&mut self, custom: HashMap<String, AttributeValue>) -> &mut Self {
        self.custom.extend(custom);
        self
    }

    /// Set a list of attributes that are considered private for this user only.
    pub fn private_attributes(&mut self, private_attributes: HashSet<String>) -> &mut Self {
        self.private_attributes = private_attributes;
        self
    }

    /// Create a new [User] instance.
    pub fn build(&self) -> User {
        User {
            _key: self.key.clone(),
            _secondary: self.secondary.clone(),
            _ip: self.ip.clone(),
            _country: self.country.clone(),
            _email: self.email.clone(),
            _first_name: self.first_name.clone(),
            _last_name: self.last_name.clone(),
            _avatar: self.avatar.clone(),
            _name: self.name.clone(),
            _anonymous: self.anonymous,
            custom: self.custom.clone(),
            private_attributes: self.private_attributes.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use maplit::{hashmap, hashset};
    use spectral::prelude::*;

    const BUCKET_TOLERANCE: f32 = 0.0000001;

    #[test]
    fn test_user_attributes_serialize_correctly() {
        let user = User::with_key("userKeyA")
            .first_name("First")
            .last_name("Last")
            .custom(hashmap! { "customKey".into() => "value".into() })
            .build();
        let private_attributes = HashSet::new();
        let attributes = UserAttributes::from_user(user, false, &private_attributes);

        let expected = r#"
{
  "key": "userKeyA",
  "firstName": "First",
  "lastName": "Last",
  "custom": {
    "customKey": "value"
  }
}
        "#
        .trim();

        assert_that!(serde_json::to_string_pretty(&attributes))
            .is_ok_containing(expected.to_string());
    }

    #[test]
    fn test_user_attributes_ignore_all_nonkey_attributes() {
        let user = User::with_key("userKeyA")
            .first_name("First")
            .last_name("Last")
            .custom(hashmap! { "customKey".into() => "value".into() })
            .build();
        let private_attributes = HashSet::new();
        let attributes = UserAttributes::from_user(user, true, &private_attributes);

        let expected = r#"
{
  "key": "userKeyA",
  "privateAttrs": [
    "firstName",
    "lastName",
    "customKey"
  ]
}
        "#
        .trim();

        assert_that!(serde_json::to_string_pretty(&attributes))
            .is_ok_containing(expected.to_string());
    }

    #[test]
    fn test_user_attributes_ignores_selected_attributes() {
        let user = User::with_key("userKeyA")
            .first_name("First")
            .last_name("Last")
            .custom(hashmap! { "customKey".into() => "value".into() })
            .build();
        let private_attributes = hashset! { "firstName".into(), "customKey".into() };
        let attributes = UserAttributes::from_user(user, false, &private_attributes);

        let expected = r#"
{
  "key": "userKeyA",
  "lastName": "Last",
  "privateAttrs": [
    "firstName",
    "customKey"
  ]
}
        "#
        .trim();

        assert_that!(serde_json::to_string_pretty(&attributes))
            .is_ok_containing(expected.to_string());
    }

    #[test]
    fn test_user_attributes_ignores_user_specified_attributes() {
        let user = User::with_key("userKeyA")
            .first_name("First")
            .last_name("Last")
            .custom(hashmap! { "customKey".into() => "value".into() })
            .private_attributes(hashset! { "firstName".into(), "customKey".into() })
            .build();
        let private_attributes = HashSet::new();
        let attributes = UserAttributes::from_user(user, false, &private_attributes);

        let expected = r#"
{
  "key": "userKeyA",
  "lastName": "Last",
  "privateAttrs": [
    "firstName",
    "customKey"
  ]
}
        "#
        .trim();

        assert_that!(serde_json::to_string_pretty(&attributes))
            .is_ok_containing(expected.to_string());
    }

    #[test]
    fn bucket_user_by_key() {
        const PREFIX: BucketPrefix = BucketPrefix::KeyAndSalt("hashKey", "saltyA");

        let user = User::with_key("userKeyA").build();
        let bucket = user.bucket(None, PREFIX);
        assert_that!(bucket).is_close_to(0.42157587, BUCKET_TOLERANCE);

        let user = User::with_key("userKeyB").build();
        let bucket = user.bucket(None, PREFIX);
        assert_that!(bucket).is_close_to(0.6708485, BUCKET_TOLERANCE);

        let user = User::with_key("userKeyC").build();
        let bucket = user.bucket(None, PREFIX);
        assert_that!(bucket).is_close_to(0.10343106, BUCKET_TOLERANCE);
    }

    #[test]
    fn bucket_user_by_key_with_seed() {
        const PREFIX: BucketPrefix = BucketPrefix::Seed(61);

        let user_a = User::with_key("userKeyA").build();
        let bucket = user_a.bucket(None, PREFIX);
        assert_that!(bucket).is_close_to(0.09801207, BUCKET_TOLERANCE);

        let user_b = User::with_key("userKeyB").build();
        let bucket = user_b.bucket(None, PREFIX);
        assert_that!(bucket).is_close_to(0.14483777, BUCKET_TOLERANCE);

        let user_c = User::with_key("userKeyC").build();
        let bucket = user_c.bucket(None, PREFIX);
        assert_that!(bucket).is_close_to(0.9242641, BUCKET_TOLERANCE);

        // changing seed produces different bucket value
        let bucket = user_a.bucket(None, BucketPrefix::Seed(60));
        assert_that!(bucket).is_close_to(0.7008816, BUCKET_TOLERANCE)
    }

    #[test]
    fn bucket_user_by_int_attr() {
        const USER_KEY: &str = "userKeyD";
        const PREFIX: BucketPrefix = BucketPrefix::KeyAndSalt("hashKey", "saltyA");

        let custom = hashmap! {
            "intAttr".into() => 33333.into(),
        };
        let user = User::with_key(USER_KEY).custom(custom).build();
        let bucket = user.bucket(Some("intAttr"), PREFIX);
        assert_that!(bucket).is_close_to(0.54771423, BUCKET_TOLERANCE);

        let custom = hashmap! {
        "stringAttr".into() => "33333".into(),
        };
        let user = User::with_key(USER_KEY).custom(custom).build();
        let bucket2 = user.bucket(Some("stringAttr"), PREFIX);
        assert_that!(bucket).is_close_to(bucket2, BUCKET_TOLERANCE);
    }

    #[test]
    fn bucket_user_by_float_attr_not_allowed() {
        const USER_KEY: &str = "userKeyE";
        const PREFIX: BucketPrefix = BucketPrefix::KeyAndSalt("hashKey", "saltyA");

        let custom = hashmap! {
            "floatAttr".into() => 999.999.into(),
        };
        let user = User::with_key(USER_KEY).custom(custom).build();
        let bucket = user.bucket(Some("floatAttr"), PREFIX);
        assert_that!(bucket).is_close_to(0.0, BUCKET_TOLERANCE);
    }

    #[test]
    fn parse_user_rejects_missing_key() {
        let result: serde_json::Result<User> = serde_json::from_str(r"{}");
        assert_that!(result).is_err();
    }

    #[test]
    fn parse_user_rejects_null_key() {
        let result: serde_json::Result<User> = serde_json::from_str(r#"{"key": null}"#);
        assert_that!(result).is_err();
    }

    #[test]
    fn null_custom_is_default() {
        let user1: User = serde_json::from_str(r#"{"key": "foo"}"#).unwrap();
        assert_eq!(user1.custom, hashmap![]);

        let user2: User = serde_json::from_str(r#"{"key": "foo", "custom": null}"#).unwrap();
        assert_eq!(user2.custom, hashmap![]);
    }

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
    fn user_attribute() {
        let mut user = User::with_key("abc").build();

        for attribute in vec![
            "key",
            "secondary",
            "ip",
            "country",
            "email",
            "firstName",
            "lastName",
            "avatar",
            "name",
        ] {
            user.attribute(attribute, "123").unwrap();
            user.attribute(attribute, 123).unwrap_err();
        }

        user.attribute("anonymous", true).unwrap();
        user.attribute("anonymous", 123).unwrap_err();
        user.attribute("custom", "123").unwrap();
        user.attribute("custom", 123).unwrap();
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
