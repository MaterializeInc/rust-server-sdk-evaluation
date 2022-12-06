use crate::contexts::attribute_reference::AttributeName;
use crate::contexts::context::Kind;
use crate::contexts::context_serde_helpers::*;
use crate::{AttributeValue, MultiContextBuilder};
use crate::{Context, ContextBuilder};
use serde::de;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_with::skip_serializing_none;
use std::collections::HashMap;
use std::convert::TryFrom;

// Represents the three possible context formats recognized by this SDK.
// Multi-kind contexts hold one or more nested contexts.
// Single-kind contexts hold a single context.
// Implicit-kind contexts represent the pre-context data format.
pub(super) enum ContextVariant {
    Multi(MultiKindContext),
    Single(SingleKindStandaloneContext),
    Implicit(UserFormat),
}

// Represents the serialization/deserialization format of a multi-kind context, which serves as a container
// for 1 or more single-kind contexts.
//
// MultiKindContext is not used directly; it is an intermediate format between JSON and
// the user-facing Context type.
#[derive(Serialize, Deserialize)]
pub(super) struct MultiKindContext {
    kind: String,
    #[serde(flatten)]
    contexts: HashMap<Kind, SingleKindContext>,
}

// Represents the serialization/deserialization format of a single-kind context that is not
// nested within a multi-kind context.
//
// SingleKindStandaloneContext is not used directly; it is an intermediate format between JSON and
// the user-facing Context type.
//
// The only difference between this representation and [SingleKindContext] is the
// requirement of a top-level 'kind' string.
//
// Single-kind contexts nested in a multi-kind context do not have these because the kind
// is a key in the multi-kind object, whereas it needs to be specified explicitly in a standalone
// single-kind context.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SingleKindStandaloneContext {
    kind: Kind,
    #[serde(flatten)]
    context: SingleKindContext,
}

// Represents the serialization/deserialization format of a single-kind context nested
// within a multi-context.
//
// SingleKindContext is not used directly; it is an intermediate format between JSON
// and the user-facing Context type.
#[skip_serializing_none]
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SingleKindContext {
    key: String,
    name: Option<String>,
    #[serde(skip_serializing_if = "is_false_bool_option")]
    anonymous: Option<bool>,
    #[serde(flatten)]
    attributes: HashMap<String, AttributeValue>,
    #[serde(rename = "_meta", skip_serializing_if = "is_none_meta_option")]
    meta: Option<Meta>,
}

// UserFormat represents the serialization/deserialization data format
// used by LaunchDarkly SDKs that do not support contexts.
//
// Any context that matches this format may be deserialized, but serialization will result
// in conversion to the single-kind context format.
#[skip_serializing_none]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UserFormat {
    key: String,
    name: Option<String>,
    secondary: Option<String>,
    anonymous: Option<bool>,
    custom: Option<HashMap<String, AttributeValue>>,
    private_attribute_names: Option<Vec<AttributeName>>,
    first_name: Option<String>,
    last_name: Option<String>,
    avatar: Option<String>,
    email: Option<String>,
    country: Option<String>,
    ip: Option<String>,
}

#[skip_serializing_none]
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Meta {
    pub secondary: Option<String>,
    #[serde(skip_serializing_if = "is_empty_vec_option")]
    pub private_attributes: Option<Vec<String>>,
}

// This deserialize method is needed to discriminate between the three possible context formats supported
// by LaunchDarkly.
//
// The code generated by serde's untagged enum feature is not sufficient. This is because there is overlap
// between the fields supported by all three formats. For example, a context with 'kind' = false
// should fail to parse. But instead, this would be deserialized as a [ContextVariant::Implicit], since it doesn't match
// [ContextVariant::Single] because kind is specified to be a string.
impl<'de> Deserialize<'de> for ContextVariant {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // The 'field_identifier' attribute is currently undocumented. See:
        // https://github.com/serde-rs/serde/issues/1221#issuecomment-382801024

        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "camelCase")]
        enum Tag {
            Multi,
            Custom(String),
        }

        let v = serde_json::Value::deserialize(deserializer)?;
        match Option::deserialize(&v["kind"]).map_err(de::Error::custom)? {
            None if v.get("kind").is_some() => {
                // Interpret this condition (None && v.get("kind").is_some()) as meaning
                // "could not deserialize 'kind' into a Tag (string), but 'kind' was still present
                // in the data." serde_json::Value::Null (assumption) is the only value that fits that
                // condition. If it were another type, like boolean, it wouldn't make it into this
                // match expression at all.
                Err(de::Error::custom("context kind cannot be null"))
            }
            None => {
                let user = UserFormat::deserialize(v).map_err(de::Error::custom)?;
                Ok(ContextVariant::Implicit(user))
            }
            Some(Tag::Multi) => {
                let multi = MultiKindContext::deserialize(v).map_err(de::Error::custom)?;
                Ok(ContextVariant::Multi(multi))
            }
            Some(Tag::Custom(kind)) if kind.is_empty() => {
                Err(de::Error::custom("context kind cannot be empty string"))
            }
            Some(Tag::Custom(_)) => {
                let single =
                    SingleKindStandaloneContext::deserialize(v).map_err(de::Error::custom)?;
                Ok(ContextVariant::Single(single))
            }
        }
    }
}

impl Serialize for ContextVariant {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            ContextVariant::Multi(multi) => multi.serialize(serializer),
            ContextVariant::Single(single) => single.serialize(serializer),
            ContextVariant::Implicit(_) => {
                unimplemented!("cannot serialize implicit user contexts")
            }
        }
    }
}

impl From<Context> for ContextVariant {
    fn from(c: Context) -> Self {
        match c.kind {
            kind if kind.is_multi() => ContextVariant::Multi(MultiKindContext {
                kind: "multi".to_owned(),
                contexts: c
                    .contexts
                    .expect("multi-kind context must contain at least one nested context")
                    .into_iter()
                    .map(single_kind_context_from)
                    .collect(),
            }),
            _ => {
                let (kind, nested) = single_kind_context_from(c);
                ContextVariant::Single(SingleKindStandaloneContext {
                    kind,
                    context: nested,
                })
            }
        }
    }
}

fn single_kind_context_from(c: Context) -> (Kind, SingleKindContext) {
    (
        c.kind,
        SingleKindContext {
            key: c.key,
            name: c.name,
            anonymous: Some(c.anonymous),
            attributes: c.attributes,
            meta: Some(Meta {
                secondary: c.secondary,
                private_attributes: c
                    .private_attributes
                    .map(|attrs| attrs.into_iter().map(String::from).collect()),
            }),
        },
    )
}

impl TryFrom<ContextVariant> for Context {
    type Error = String;

    fn try_from(variant: ContextVariant) -> Result<Self, Self::Error> {
        match variant {
            ContextVariant::Multi(m) => {
                let mut multi_builder = MultiContextBuilder::new();
                for (kind, context) in m.contexts {
                    let mut builder = ContextBuilder::new(context.key.clone());
                    let context = build_context(&mut builder, context).kind(kind).build()?;
                    multi_builder.add_context(context);
                }
                multi_builder.build()
            }
            ContextVariant::Single(context) => {
                let mut builder = ContextBuilder::new(context.context.key.clone());
                build_context(&mut builder, context.context)
                    .kind(context.kind)
                    .build()
            }
            ContextVariant::Implicit(user) => {
                let mut builder = ContextBuilder::new(user.key.clone());
                builder.allow_empty_key();
                build_context_from_implicit_user(&mut builder, user).build()
            }
        }
    }
}

fn build_context(b: &mut ContextBuilder, context: SingleKindContext) -> &mut ContextBuilder {
    for (key, attr) in context.attributes {
        b.set_value(key.as_str(), attr);
    }
    if let Some(anonymous) = context.anonymous {
        b.anonymous(anonymous);
    }
    if let Some(name) = context.name {
        b.name(name);
    }
    if let Some(meta) = context.meta {
        if let Some(secondary) = meta.secondary {
            b.secondary(secondary);
        }
        if let Some(private_attributes) = meta.private_attributes {
            for attribute in private_attributes {
                b.add_private_attribute(attribute);
            }
        }
    }
    b
}

// This is used when unmarshalling an old-style UserFormat into a Context.
// If we see any of these names within the "custom": {} object, logically
// we shouldn't use ContextBuilder::set_value to set it because that could overwrite
// any top-level attributes of the same name.
fn should_skip_custom_attribute(attr_name: &str) -> bool {
    matches!(attr_name, "kind" | "key" | "name" | "anonymous" | "_meta")
}

fn build_context_from_implicit_user(
    b: &mut ContextBuilder,
    user: UserFormat,
) -> &mut ContextBuilder {
    if let Some(anonymous) = user.anonymous {
        b.anonymous(anonymous);
    }
    if let Some(secondary) = user.secondary {
        b.secondary(secondary);
    }
    if let Some(name) = user.name {
        b.name(name);
    }
    if let Some(x) = user.avatar {
        b.set_string("avatar", x);
    }
    if let Some(x) = user.first_name {
        b.set_string("firstName", x);
    }
    if let Some(x) = user.last_name {
        b.set_string("lastName", x);
    }
    if let Some(x) = user.country {
        b.set_string("country", x);
    }
    if let Some(x) = user.email {
        b.set_string("email", x);
    }
    if let Some(x) = user.ip {
        b.set_string("ip", x);
    }
    if let Some(custom) = user.custom {
        for (key, attr) in custom {
            if !should_skip_custom_attribute(&key) {
                b.set_value(key.as_str(), attr);
            }
        }
    }
    if let Some(attributes) = user.private_attribute_names {
        for attribute in attributes {
            b.add_private_attribute(attribute);
        }
    }
    b
}

#[cfg(test)]
mod tests {
    use crate::contexts::context_serde::{ContextVariant, UserFormat};
    use crate::{AttributeValue, Context, ContextBuilder, MultiContextBuilder};
    use assert_json_diff::assert_json_eq;
    use maplit::hashmap;
    use serde_json::json;
    use std::convert::TryFrom;
    use test_case::test_case;

    #[test_case(json!({"key" : "foo"}),
                json!({"kind" : "user", "key" : "foo"}))]
    #[test_case(json!({"key" : "foo", "name" : "bar"}),
                json!({"kind" : "user", "key" : "foo", "name" : "bar"}))]
    #[test_case(json!({"key" : "foo", "custom" : {"a" : "b"}}),
                json!({"kind" : "user", "key" : "foo", "a" : "b"}))]
    #[test_case(json!({"key" : "foo", "anonymous" : true}),
                json!({"kind" : "user", "key" : "foo", "anonymous" : true}))]
    #[test_case(json!({"key" : "foo", "secondary" : "bar"}),
                json!({"kind" : "user", "key" : "foo", "_meta" : {"secondary" : "bar"}}))]
    #[test_case(json!({"key" : "foo", "ip" : "1", "privateAttributeNames" : ["ip"]}),
                json!({"kind" : "user", "key" : "foo", "ip" : "1", "_meta" : { "privateAttributes" : ["ip"]} }))]
    // Don't let custom attributes overwrite top-level properties with the same reserved names
    #[test_case(
        json!({
            "key" : "foo",
            "name" : "bar",
            "anonymous" : true,
            "custom" : {
                "kind": ".",
                "key": ".",
                "name": ".",
                "anonymous": true,
                "_meta": true,
                "a": 1.0
            }
        }),
        json!({
            "kind": "user",
            "key": "foo",
            "name": "bar",
            "anonymous": true,
            "a": 1.0
        })
    )]
    // This test ensures that contexts in the implicit user format are converted into single-kind format.
    // This involves various transformations, such as converting privateAttributeNames into a _meta key privateAttributes.
    fn implicit_context_conversion(from: serde_json::Value, to: serde_json::Value) {
        let context: Result<ContextVariant, _> = serde_json::from_value(from);
        match context {
            Ok(variant) => {
                assert!(matches!(variant, ContextVariant::Implicit(_)));
                match Context::try_from(variant) {
                    Ok(context) => {
                        assert_json_eq!(to, context);
                    }
                    Err(e) => panic!("variant should convert to context without error: {:?}", e),
                }
            }
            Err(e) => panic!("test JSON should parse without error: {:?}", e),
        }
    }

    #[test_case(json!({"kind" : "org", "key" : "foo"}))]
    #[test_case(json!({"kind" : "user", "key" : "foo"}))]
    #[test_case(json!({"kind" : "foo", "key" : "bar", "anonymous" : true}))]
    #[test_case(json!({"kind" : "foo", "name" : "Foo", "key" : "bar", "a" : "b", "_meta" : {"secondary" : "baz", "privateAttributes" : ["a"]}}))]
    #[test_case(json!({"kind" : "foo", "key" : "bar", "object" : {"a" : "b"}}))]
    // This test ensures that single-kinded contexts are deserialized and then reserialized without any
    // changes.
    fn single_kind_context_roundtrip_identical(from: serde_json::Value) {
        match serde_json::from_value::<Context>(from.clone()) {
            Ok(context) => {
                assert_json_eq!(from, context);
            }
            Err(e) => panic!("test JSON should convert to context without error: {:?}", e),
        }
    }

    #[test_case(json!({"kind" : true, "key" : "a"}))]
    #[test_case(json!({"kind" : null, "key" : "b"}))]
    #[test_case(json!({"kind" : {}, "key" : "c"}))]
    #[test_case(json!({"kind" : 1, "key" : "d"}))]
    #[test_case(json!({"kind" : [], "key" : "e"}))]
    fn reject_null_or_non_string_kind(from: serde_json::Value) {
        match serde_json::from_value::<ContextVariant>(from) {
            Err(e) => println!("{:?}", e),
            Ok(c) => panic!(
                "expected conversion to fail, but got: {:?}",
                serde_json::to_string(&c)
            ),
        }
    }

    // Kind cannot be 'kind'
    #[test_case(json!({"kind" : "kind", "key" : "a"}))]
    #[test_case(json!({"kind" : "", "key" : "a"}))]
    // Multi-kind must have at least one nested kind
    #[test_case(json!({"kind" : "multi"}))]
    #[test_case(json!({"kind" : "multi", "key" : "a"}))]
    // Single-kind, if contains _meta key, must be an object
    #[test_case(json!({"kind" : "user", "key" : "a", "_meta" : "string"}))]
    // Context must either be implicit user, single, or multi.
    #[test_case(json!({"a" : "b"}))]
    // Single kind must contain key.
    #[test_case(json!({"kind" : "user"}))]
    #[test_case(json!({"kind" : "user", "key" : ""}))]
    fn reject_invalid_contexts(from: serde_json::Value) {
        match serde_json::from_value::<Context>(from) {
            Err(e) => println!("got expected error: {:?}", e),
            Ok(c) => panic!(
                "expected conversion to fail, but got: {:?}",
                serde_json::to_string(&c)
            ),
        }
    }

    #[test]
    // An empty key is only allowed for implicit user format for backwards compatability reasons.
    fn empty_key_allowed_for_implicit_user() {
        let json_in = json!({
            "key" : "",
        });

        let json_out = json!({
            "kind" : "user",
            "key" : ""
        });

        let context: Context = serde_json::from_value(json_in).unwrap();

        assert_json_eq!(json_out, json!(context));
    }

    #[test]
    // The deserialization algorithm should be able to ignore unrecognized top-level
    // property names without generating an error.
    fn unrecognized_implicit_user_props_are_ignored_without_error() {
        let json = json!({
            "key" : "foo",
            "ip": "b",
            "unknown-1" : "ignored",
            "unknown-2" : "ignored",
            "unknown-3" : "ignored",
        });

        match serde_json::from_value::<ContextVariant>(json) {
            Err(e) => panic!("expected user to deserialize without error: {:?}", e),
            Ok(c) => match c {
                ContextVariant::Implicit(user) => {
                    assert_eq!(user.ip.unwrap_or_else(|| "".to_string()), "b");
                }
                _ => panic!("expected user format"),
            },
        }
    }

    #[test]
    fn multi_kind_context_roundtrip() {
        let json = json!({
            "kind" : "multi",
            "foo" : {
                "key" : "foo_key",
                "name" : "foo_name",
                "anonymous" : true
            },
            "bar" : {
                "key" : "bar_key",
                "some" : "attribute",
                "_meta" : {
                    "secondary" : "bar_two",
                    "privateAttributes" : [
                        "some"
                    ]
                }
            },
            "baz" : {
                "key" : "baz_key",
            }
        });

        let multi: Context = serde_json::from_value(json.clone()).unwrap();
        assert_json_eq!(multi, json);
    }

    #[test]
    fn builder_generates_correct_single_kind_context() {
        let json = json!({
            "kind" : "org",
            "key" : "foo",
            "anonymous" : true,
            "_meta" : {
                "privateAttributes" : ["a", "b", "/c/d"],
                "secondary" : "bar"
            },
            "a" : true,
            "b" : true,
            "c" : {
                "d" : "e"
            }
        });

        let mut builder = ContextBuilder::new("foo");
        let result = builder
            .anonymous(true)
            .secondary("bar")
            .kind("org")
            .set_bool("a", true)
            .add_private_attribute("a")
            .set_bool("b", true)
            .add_private_attribute("b")
            .set_value(
                "c",
                AttributeValue::Object(hashmap! {
                    "d".into() => "e".into()
                }),
            )
            .add_private_attribute("/c/d")
            .build()
            .unwrap();

        assert_json_eq!(json, result);
    }

    #[test]
    fn build_generates_correct_multi_kind_context() {
        let json = json!({
            "kind" : "multi",
            "user" : {
                "key" : "foo-key",
            },
            "bar" : {
                "key" : "bar-key",
            },
            "baz" : {
                "key" : "baz-key",
                "anonymous" : true
            }
        });

        let user = ContextBuilder::new("foo-key");
        let mut bar = ContextBuilder::new("bar-key");
        bar.kind("bar");

        let mut baz = ContextBuilder::new("baz-key");
        baz.kind("baz");
        baz.anonymous(true);

        let multi = MultiContextBuilder::new()
            .add_context(user.build().expect("failed to build context"))
            .add_context(bar.build().expect("failed to build context"))
            .add_context(baz.build().expect("failed to build context"))
            .build()
            .unwrap();

        assert_json_eq!(multi, json);
    }

    #[test]
    #[should_panic]
    // Implicit user contexts should never be serialized. All deserialized implicit
    // user contexts should be re-serialized as single-kind contexts with kind=user.
    fn cannot_serialize_implicit_user_context() {
        let x = ContextVariant::Implicit(UserFormat {
            key: "foo".to_string(),
            name: None,
            secondary: None,
            anonymous: None,
            custom: None,
            private_attribute_names: None,
            first_name: None,
            last_name: None,
            avatar: None,
            email: None,
            country: None,
            ip: None,
        });

        let _ = serde_json::to_string(&x);
    }
}