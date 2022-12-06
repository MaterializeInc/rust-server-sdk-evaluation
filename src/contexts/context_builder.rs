use super::{attribute_reference::Reference, context::Context, context::Kind};
use crate::AttributeValue;
use log::warn;

use std::collections::HashMap;
use urlencoding::encode;

const DEFAULT_MULTI_BUILDER_CAPACITY: usize = 3; // arbitrary value based on presumed likely use cases

/// Contains methods for building a [Context] with a specified key.
///
/// To define a multi-context (containing more than one kind) see [MultiContextBuilder].
///
/// You may use these methods to set additional attributes and/or change the kind before calling
/// [ContextBuilder::build]. If you do not change any values, the defaults for the [Context] are:
/// - its kind is "user"
/// - its key is set to whatever value you passed to [ContextBuilder::new]
/// - its [anonymous attribute](ContextBuilder::anonymous) is `false`
/// - it has no values for any other attributes.
pub struct ContextBuilder {
    kind: String,
    name: Option<String>,
    anonymous: bool,
    secondary: Option<String>,
    private_attributes: Vec<Reference>,

    key: String,
    attributes: HashMap<String, AttributeValue>,

    // Contexts that were deserialized from implicit user format
    // are allowed to have empty string keys. Otherwise,
    // key is never allowed to be empty.
    allow_empty_key: bool,
}

impl ContextBuilder {
    /// Create a new context builder with the provided "key" attribute.
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            kind: "user".to_owned(),
            name: None,
            anonymous: false,
            secondary: None,
            private_attributes: Vec::new(),
            key: key.into(),
            attributes: HashMap::new(),
            allow_empty_key: false,
        }
    }

    /// Sets the context's "kind" attribute, which is "user" by default.
    ///
    /// Validation rules are as follows:
    /// - It may not be an empty string
    /// - It may only contain letters, numbers, and the characters `.`, `_`, and `-`
    /// - It cannot be "kind"
    /// - It cannot be "multi"
    ///
    /// If the value is invalid, you will receive an error when [ContextBuilder::build] is called.
    ///
    /// To ensure that a given kind will be valid, you may use [Kind::try_from] and pass that here.
    pub fn kind(&mut self, kind: impl Into<String>) -> &mut Self {
        self.kind = kind.into();
        self
    }

    /// Sets the Context's key attribute. The provided key cannot be an empty string.
    ///
    /// The key attribute can be referenced by flag rules, flag target
    /// lists, and segments.
    pub fn key(&mut self, key: impl Into<String>) -> &mut Self {
        self.key = key.into();
        self
    }

    /// Sets the context's "name" attribute.
    ///
    /// This attribute is optional. It has the following special rules:
    ///
    /// - Unlike most other attributes, it is always a string if it is specified.
    /// - The LaunchDarkly dashboard treats this attribute as the preferred display name for users.
    pub fn name(&mut self, name: impl Into<String>) -> &mut Self {
        self.name = Some(name.into());
        self
    }

    /// Sets an attribute to a boolean value.
    ///
    /// For rules regarding attribute names and values, see [ContextBuilder::set_value]. This method is
    /// exactly equivalent to calling `self.set_value(attribute_name,
    /// AttributeValue::Bool(value))`.
    pub fn set_bool(&mut self, attribute_name: &str, value: bool) -> &mut Self {
        self.set_value(attribute_name, AttributeValue::Bool(value));
        self
    }

    /// Sets an attribute to a f64 numeric value.
    ///
    /// For rules regarding attribute names and values, see [ContextBuilder::set_value]. This method is
    /// exactly equivalent to calling `self.set_value(attribute_name,
    /// AttributeValue::Number(value))`.
    ///
    /// Note: the LaunchDarkly model for feature flags and context attributes is based on JSON types,
    /// and does not distinguish between integer and floating-point types.
    pub fn set_float(&mut self, attribute_name: &str, value: f64) -> &mut Self {
        self.set_value(attribute_name, AttributeValue::Number(value));
        self
    }

    /// Sets an attribute to a string value.
    ///
    /// For rules regarding attribute names and values, see [ContextBuilder::set_value]. This method is
    /// exactly equivalent to calling `self.set_value(attribute_name,
    /// AttributeValue::String(value.to_string()))`.
    pub fn set_string(&mut self, attribute_name: &str, value: impl Into<String>) -> &mut Self {
        self.set_value(attribute_name, AttributeValue::String(value.into()));
        self
    }

    /// Sets the value of any attribute for the context.
    ///
    /// This includes only attributes that are addressable in evaluations -- not metadata such as
    /// private attributes. For example, if `attribute_name` is "privateAttributes", you will be
    /// setting an attribute with that name which you can use in evaluations or to record data for
    /// your own purposes, but it will be unrelated to [ContextBuilder::add_private_attribute].
    ///
    /// If `attribute_name` is "privateAttributeNames", it is ignored and no
    /// attribute is set.
    ///
    /// This method uses the [AttributeValue] type to represent a value of any JSON type: null,
    /// boolean, number, string, array, or object. For all attribute names that do not have special
    /// meaning to LaunchDarkly, you may use any of those types. Values of different JSON types are
    /// always treated as different values: for instance, null, false, and the empty string "" are
    /// not the same, and the number 1 is not the same as the string "1".
    ///
    /// The following attribute names have special restrictions on their value types, and any value
    /// of an unsupported type will be ignored (leaving the attribute unchanged):
    ///
    /// - "kind", "key": Must be a string. See [ContextBuilder::kind] and [ContextBuilder::key].
    ///
    /// - "name": Must be a string. See [ContextBuilder::name].
    ///
    /// - "anonymous": Must be a boolean. See [ContextBuilder::anonymous].
    ///
    /// The attribute name "_meta" is not allowed, because it has special meaning in the JSON
    /// schema for contexts; any attempt to set an attribute with this name has no effect.
    ///
    /// Values that are JSON arrays or objects have special behavior when referenced in
    /// flag/segment rules.
    ///
    /// For attributes that aren't subject to the special restrictions mentioned above,
    /// a value of [AttributeValue::Null] is equivalent to removing any current non-default value
    /// of the attribute. Null is not a valid attribute value in the LaunchDarkly model; any
    /// expressions in feature flags that reference an attribute with a null value will behave as
    /// if the attribute did not exist.
    pub fn set_value(&mut self, attribute_name: &str, value: AttributeValue) -> &mut Self {
        let _ = self.try_set_value(attribute_name, value);
        self
    }

    /// Sets the value of any attribute for the context.
    ///
    /// This is the same as [ContextBuilder::set_value], except that it returns true for success, or false if
    /// the parameters violated one of the restrictions described for [ContextBuilder::set_value] (for
    /// instance, attempting to set "key" to a value that was not a string).
    pub fn try_set_value(&mut self, attribute_name: &str, value: AttributeValue) -> bool {
        match (attribute_name, value.clone()) {
            ("", _) => {
                warn!("Provided attribute name is empty. Ignoring.");
                false
            }
            ("kind", AttributeValue::String(s)) => {
                self.kind(s);
                true
            }
            ("kind", _) => false,
            ("key", AttributeValue::String(s)) => {
                self.key(s);
                true
            }
            ("key", _) => false,
            ("name", AttributeValue::String(s)) => {
                self.name(s);
                true
            }
            ("name", AttributeValue::Null) => {
                self.name = None;
                true
            }
            ("name", _) => false,
            ("anonymous", AttributeValue::Bool(b)) => {
                self.anonymous(b);
                true
            }
            ("anonymous", _) => false,
            ("_meta", _) => false,
            (_, AttributeValue::Null) => {
                self.attributes.remove(attribute_name);
                true
            }
            (_, _) => {
                self.attributes.insert(attribute_name.to_string(), value);
                true
            }
        }
    }

    /// Sets a secondary key for the context.
    ///
    /// This corresponds to the "secondary" attribute in the older LaunchDarkly user schema. Since
    /// LaunchDarkly still supports evaluating feature flags for old-style users, this attribute is
    /// still available to the evaluation logic if it was present in user JSON and the
    /// 'secondary_key_bucketing' Cargo feature flag is enabled, but it cannot be intentionally set
    /// by external users via the builder API.
    ///
    /// Setting this value to an empty string is not the same as leaving it unset.
    pub(in crate::contexts) fn secondary(&mut self, value: impl Into<String>) -> &mut Self {
        self.secondary = Some(value.into());
        self
    }

    /// Designates any number of context attributes as private: that is, their values will not
    /// be sent to LaunchDarkly.
    ///
    /// See [Reference] for details on how to construct a valid reference.
    ///
    /// This action only affects analytics events that involve this particular context. To mark some (or all)
    /// context attributes as private for all uses, use the overall event configuration for the SDK.
    ///
    /// The attributes "kind" and "key", and the metadata property set by [ContextBuilder::anonymous],
    /// cannot be made private.
    pub fn add_private_attribute<R: Into<Reference>>(&mut self, reference: R) -> &mut Self {
        self.private_attributes.push(reference.into());
        self
    }

    /// Remove any reference provided through [ContextBuilder::add_private_attribute]. If the reference was
    /// added more than once, this method will remove all instances of it.
    pub fn remove_private_attribute<R: Into<Reference>>(&mut self, reference: R) -> &mut Self {
        let reference = reference.into();
        self.private_attributes
            .retain(|private| *private != reference);
        self
    }

    /// Sets whether the context is only intended for flag evaluations and should not be indexed by
    /// LaunchDarkly.
    ///
    /// The default value is `false`, which means that this context represents an entity such as a
    /// user that you want to see on the LaunchDarkly dashboard.
    ///
    /// Setting anonymous to `true` excludes this context from the database that is used by the
    /// dashboard. It does not exclude it from analytics event data, so it is not the same as
    /// making attributes private; all non-private attributes will still be included in events and
    /// data export.
    ///
    /// This value is also addressable in evaluations as the attribute name "anonymous".
    pub fn anonymous(&mut self, value: bool) -> &mut Self {
        self.anonymous = value;
        self
    }

    /// Allows the context to have an empty string key. This is for backwards compatability purposes
    /// when deserializing implicit user contexts, and is only used internally.
    pub(super) fn allow_empty_key(&mut self) -> &mut Self {
        self.allow_empty_key = true;
        self
    }

    /// Creates a context from the current builder's properties.
    ///
    /// The context is immutable and will not be affected by any subsequent actions on the
    /// builder.
    ///
    /// It is possible to specify invalid attributes for a builder, such as an empty key.
    /// In those situations, an `Err` type will be returned.
    pub fn build(&self) -> Result<Context, String> {
        let kind = Kind::try_from(self.kind.clone())?;

        if kind.is_multi() {
            return Err(String::from(
                "context of kind \"multi\" must be built with MultiContextBuilder",
            ));
        }

        if !self.allow_empty_key && self.key.is_empty() {
            return Err(String::from("key cannot be empty"));
        }

        let canonical_key = canonical_key_for_kind(&kind, &self.key, true);

        Ok(Context {
            kind,
            contexts: None,
            name: self.name.clone(),
            anonymous: self.anonymous,
            secondary: self.secondary.clone(),
            private_attributes: if self.private_attributes.is_empty() {
                None
            } else {
                Some(self.private_attributes.clone())
            },
            key: self.key.clone(),
            canonical_key,
            attributes: self.attributes.clone(),
        })
    }
}

fn canonical_key_for_kind(kind: &Kind, key: &str, omit_user_kind: bool) -> String {
    if omit_user_kind && kind.is_user() {
        return key.to_owned();
    }
    format!("{}:{}", kind, encode(key))
}

/// Contains methods for building a multi-context.
///
/// Use this builder if you need to construct a context that has multiple kinds, each representing their
/// own [Context]. Otherwise, use [ContextBuilder].
///
/// Obtain an instance of the builder by calling [MultiContextBuilder::new]; then, call
/// [MultiContextBuilder::add_context] to add a kind.
/// [MultiContextBuilder] setters return a reference the same builder, so they can be chained
/// together.
pub struct MultiContextBuilder {
    contexts: Vec<Context>,
}

impl MultiContextBuilder {
    /// Create a new multi-context builder. An empty builder cannot create a valid [Context]; you must
    /// add one or more kinds via [MultiContextBuilder::add_context].
    ///
    /// If you already have a list of contexts, you can instead use [MultiContextBuilder::of].
    pub fn new() -> Self {
        Self {
            contexts: Vec::with_capacity(DEFAULT_MULTI_BUILDER_CAPACITY),
        }
    }

    /// Create a new multi-context builder from the given list of contexts.
    pub fn of(contexts: Vec<Context>) -> Self {
        let mut this = MultiContextBuilder::new();
        for c in contexts {
            this.add_context(c);
        }
        this
    }

    /// Adds a context to the builder.
    ///
    /// It is invalid to add more than one context of the same [Kind]. This error is detected when
    /// you call [MultiContextBuilder::build].
    ///
    /// If `context` is a multi-context, this is equivalent to adding each individual
    /// context.
    pub fn add_context(&mut self, context: Context) -> &mut Self {
        let mut contexts = match context.contexts {
            Some(multi) => multi,
            None => vec![context],
        };
        self.contexts.append(&mut contexts);
        self
    }

    /// Creates a context from the builder's current properties.
    ///
    /// The context is immutable and will not be affected by any subsequent actions on the
    /// [MultiContextBuilder].
    ///
    /// It is possible for a [MultiContextBuilder] to represent an invalid state. In those
    /// situations, an `Err` type will be returned.
    ///
    /// If only one context was added to the builder, this method returns that context.
    pub fn build(&self) -> Result<Context, String> {
        if self.contexts.is_empty() {
            return Err("multi-kind context must contain at least one nested context".into());
        }

        if self.contexts.len() == 1 {
            return Ok(self.contexts[0].clone());
        }

        let mut contexts = self.contexts.clone();
        contexts.sort_by(|a, b| a.kind.cmp(&b.kind));
        for (index, context) in contexts.iter().enumerate() {
            if index > 0 && contexts[index - 1].kind == context.kind {
                return Err("multi-kind context cannot have same kind more than once".into());
            }
        }

        let canonicalized_key = contexts
            .iter()
            .map(|context| canonical_key_for_kind(context.kind(), context.key(), false))
            .collect::<Vec<_>>()
            .join(":");

        Ok(Context {
            kind: Kind::multi(),
            contexts: Some(contexts),
            name: None,
            anonymous: false,
            secondary: None,
            private_attributes: None,
            key: "".to_owned(),
            canonical_key: canonicalized_key,
            attributes: HashMap::new(),
        })
    }
}

impl Default for MultiContextBuilder {
    /// Creates an empty multi-context builder. In this state, a context must
    /// be [added](MultiContextBuilder::add_context) in order to build successfully.
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{ContextBuilder, MultiContextBuilder};
    use crate::{AttributeValue, Reference};

    use crate::contexts::context::Kind;
    use test_case::test_case;

    #[test]
    fn builder_can_create_correct_context() {
        let mut builder = ContextBuilder::new("key");
        builder
            .kind(Kind::user())
            .name("Name")
            .secondary("Secondary")
            .anonymous(true);

        let context = builder.build().expect("Failed to build context");

        assert!(!context.is_multi());
        assert!(context.kind.is_user());
        assert_eq!(Some("Name".to_string()), context.name);
        assert_eq!(Some("Secondary".to_string()), context.secondary);
        assert!(context.anonymous);
    }

    #[test_case("key", Kind::user(), "key")]
    #[test_case("key", Kind::from("org"), "org:key")]
    #[test_case("hi:there", Kind::user(), "hi:there")]
    #[test_case("hi:there", Kind::from("org"), "org:hi%3Athere")]
    fn builder_sets_canonical_key_correctly_for_single_context(
        key: &str,
        kind: Kind,
        expected_key: &str,
    ) {
        let context = ContextBuilder::new(key)
            .kind(kind)
            .build()
            .expect("Failed to create context");

        assert_eq!(expected_key, context.canonical_key());
    }

    // no 'user:' prefix because a multi-kind context with a single kind is built
    // as a single-kind context (special case.)
    #[test_case(vec![("key", Kind::user())], "key")]
    #[test_case(vec![("userKey", Kind::user()), ("orgKey", Kind::from("org"))], "org:orgKey:user:userKey")]
    #[test_case(vec![("some user", Kind::user()), ("org:key", Kind::from("org"))], "org:org%3Akey:user:some%20user")]
    fn builder_sets_canonical_key_correctly_for_multiple_contexts(
        tuples: Vec<(&str, Kind)>,
        expected_key: &str,
    ) {
        let mut multi_builder = MultiContextBuilder::new();

        for (key, kind) in tuples {
            multi_builder.add_context(
                ContextBuilder::new(key)
                    .kind(kind)
                    .build()
                    .expect("Failed to create context"),
            );
        }

        let multi_context = multi_builder.build().expect("Failed to create context");
        assert_eq!(expected_key, multi_context.canonical_key());
    }

    #[test_case("multi"; "Cannot set kind as multi")]
    #[test_case("kind"; "Cannot set kind as kind")]
    #[test_case("🦀"; "Cannot set kind as invalid character")]
    #[test_case(" "; "Cannot set kind as only whitespace")]
    fn build_fails_on_invalid_kinds(kind: &str) {
        let mut builder = ContextBuilder::new("key");
        builder.kind(kind);

        let result = builder.build();
        assert!(result.is_err());
    }

    #[test]
    fn builder_can_set_custom_properties_by_type() {
        let mut builder = ContextBuilder::new("key");
        builder
            .kind(Kind::user())
            .set_bool("loves-rust", true)
            .set_float("pi", 3.1459)
            .set_string("company", "LaunchDarkly");

        let context = builder.build().expect("Failed to build context");

        assert_eq!(
            &AttributeValue::Bool(true),
            context.attributes.get("loves-rust").unwrap()
        );
        assert_eq!(
            &AttributeValue::Number(3.1459),
            context.attributes.get("pi").unwrap()
        );
        assert_eq!(
            &AttributeValue::String("LaunchDarkly".to_string()),
            context.attributes.get("company").unwrap()
        );
    }

    #[test_case("", AttributeValue::Bool(true), false)]
    #[test_case("kind", AttributeValue::Bool(true), false)]
    #[test_case("kind", AttributeValue::String("user".to_string()), true)]
    #[test_case("key", AttributeValue::Bool(true), false)]
    #[test_case("key", AttributeValue::String("key".to_string()), true)]
    #[test_case("name", AttributeValue::Bool(true), false)]
    #[test_case("name", AttributeValue::String("name".to_string()), true)]
    #[test_case("anonymous", AttributeValue::String("anonymous".to_string()), false)]
    #[test_case("anonymous", AttributeValue::Bool(true), true)]
    #[test_case("secondary", AttributeValue::String("secondary".to_string()), true)]
    #[test_case("secondary", AttributeValue::Bool(true), true)]
    #[test_case("my-custom-attribute", AttributeValue::Bool(true), true)]
    #[test_case("my-custom-attribute", AttributeValue::String("string name".to_string()), true)]
    fn builder_try_set_value_handles_invalid_values_correctly(
        attribute_name: &str,
        value: AttributeValue,
        expected: bool,
    ) {
        let mut builder = ContextBuilder::new("key");
        assert_eq!(builder.try_set_value(attribute_name, value), expected);
    }

    #[test_case("secondary", AttributeValue::String("value".to_string()))]
    #[test_case("privateAttributes", AttributeValue::Array(vec![AttributeValue::String("value".to_string())]))]
    fn builder_set_value_cannot_set_meta_properties(attribute_name: &str, value: AttributeValue) {
        let builder = ContextBuilder::new("key")
            .set_value(attribute_name, value.clone())
            .build()
            .unwrap();

        assert_eq!(&value, builder.attributes.get(attribute_name).unwrap());
        assert!(builder.secondary.is_none());
    }

    #[test]
    fn builder_try_set_value_cannot_set_meta() {
        let mut builder = ContextBuilder::new("key");

        assert!(!builder.try_set_value("_meta", AttributeValue::String("value".to_string())));
        assert_eq!(builder.build().unwrap().attributes.len(), 0);
    }

    #[test]
    fn builder_deals_with_missing_kind_correctly() {
        let mut builder = ContextBuilder::new("key");
        assert!(builder.build().unwrap().kind.is_user());

        builder.kind("");
        assert!(builder.build().is_err());
    }

    #[test]
    fn builder_deals_with_empty_key_correctly() {
        assert!(ContextBuilder::new("").build().is_err());
    }

    #[test]
    fn builder_handles_private_attributes() {
        let mut builder = ContextBuilder::new("key");
        let context = builder.build().expect("Failed to build context");

        assert!(context.private_attributes.is_none());

        builder.add_private_attribute("name");
        let context = builder.build().expect("Failed to build context");

        let private = context
            .private_attributes
            .expect("Private attributes should be set");

        assert_eq!(1, private.len());
    }

    #[test]
    fn builder_handles_removing_private_attributes() {
        let mut builder = ContextBuilder::new("key");
        builder
            .add_private_attribute("name")
            .add_private_attribute("name")
            .add_private_attribute("/user/email");

        assert_eq!(
            3,
            builder.build().unwrap().private_attributes.unwrap().len()
        );

        // Removing an attribute should remove all of them.
        builder.remove_private_attribute("name");
        assert_eq!(
            1,
            builder.build().unwrap().private_attributes.unwrap().len()
        );

        builder.remove_private_attribute("/user/email");
        assert!(builder.build().unwrap().private_attributes.is_none());
    }

    #[test]
    fn build_can_add_and_remove_with_different_formats() {
        let mut builder = ContextBuilder::new("key");
        builder
            .add_private_attribute("name")
            .add_private_attribute(Reference::new("name"));

        assert_eq!(
            2,
            builder.build().unwrap().private_attributes.unwrap().len()
        );

        // Removing an attribute should remove all of them.
        builder.remove_private_attribute("name");
        assert!(builder.build().unwrap().private_attributes.is_none());

        // Removing using an attribute should also remove them all.
        builder
            .add_private_attribute("name")
            .add_private_attribute(Reference::new("name"));
        builder.remove_private_attribute(Reference::new("name"));
        assert!(builder.build().unwrap().private_attributes.is_none());
    }

    #[test]
    fn multi_builder_can_build_multi_context() {
        let mut single_builder = ContextBuilder::new("key");
        single_builder.kind(Kind::user());
        let mut multi_builder = MultiContextBuilder::new();

        multi_builder.add_context(single_builder.build().expect("Failed to build context"));

        single_builder.key("second-key").kind("org".to_string());
        multi_builder.add_context(single_builder.build().expect("Failed to build context"));

        let multi_context = multi_builder
            .build()
            .expect("Failed to create multi context");

        assert!(multi_context.is_multi());
        assert_eq!(2, multi_context.contexts.unwrap().len());
    }

    #[test]
    fn multi_builder_cannot_handle_more_than_one_of_same_kind() {
        let mut single_builder = ContextBuilder::new("key");
        single_builder.kind(Kind::user());
        let mut multi_builder = MultiContextBuilder::new();

        multi_builder.add_context(single_builder.build().expect("Failed to build context"));

        single_builder.key("second-key");
        multi_builder.add_context(single_builder.build().expect("Failed to build context"));

        let result = multi_builder.build();
        assert!(result.is_err());
    }

    #[test]
    fn multi_builder_must_contain_another_context() {
        assert!(MultiContextBuilder::new().build().is_err());
    }

    #[test]
    fn multi_builder_should_flatten_multi_contexts() {
        let cat = ContextBuilder::new("c")
            .kind("cat")
            .build()
            .expect("should build cat");

        let dog = ContextBuilder::new("d")
            .kind("dog")
            .build()
            .expect("should build dog");

        let rabbit = ContextBuilder::new("r")
            .kind("rabbit")
            .build()
            .expect("should build rabbit");

        let ferret = ContextBuilder::new("f")
            .kind("ferret")
            .build()
            .expect("should build ferret");

        let catdog = MultiContextBuilder::of(vec![cat, dog])
            .build()
            .expect("should build cat/dog multi-context");

        let rabbitferret = MultiContextBuilder::of(vec![rabbit, ferret])
            .build()
            .expect("should build rabbit/ferret multi-context");

        let chimera = MultiContextBuilder::of(vec![catdog, rabbitferret])
            .build()
            .expect("should build cat/dog/rabbit/ferret multi-context");

        assert_eq!(chimera.kinds().len(), 4);
        for k in &["cat", "dog", "rabbit", "ferret"] {
            assert!(chimera.as_kind(&Kind::from(k)).is_some());
        }
        assert_eq!(chimera.canonical_key(), "cat:c:dog:d:ferret:f:rabbit:r");
    }
}
