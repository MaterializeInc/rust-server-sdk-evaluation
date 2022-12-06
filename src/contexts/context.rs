use super::attribute_reference::Reference;
use crate::contexts::context_serde::ContextVariant;
use crate::AttributeValue;
use itertools::Itertools;
use log::warn;
use maplit::hashmap;
use serde::de::Error;
use serde::ser::SerializeMap;
use serde::{ser, Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::borrow::{Cow, ToOwned};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::convert::{TryFrom, TryInto};
use std::fmt;
use std::fmt::Formatter;
use std::string::ToString;

const BUCKET_SCALE_INT: i64 = 0x0FFF_FFFF_FFFF_FFFF;
const BUCKET_SCALE: f32 = BUCKET_SCALE_INT as f32;

/// Kind describes the type of entity represented by a [Context].
/// The meaning of a kind is entirely up to the application. To construct a custom kind other than
/// ["user"](Kind::user), see [Kind::try_from].
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct Kind(Cow<'static, str>);

impl Kind {
    /// Returns true if the kind is "user". Users are the default context kind created by [crate::ContextBuilder].
    pub fn is_user(&self) -> bool {
        self == "user"
    }

    /// Returns true if the kind is "multi". Multi-contexts are created by [crate::MultiContextBuilder].
    pub fn is_multi(&self) -> bool {
        self == "multi"
    }

    /// Constructs a kind of type "user". See also [Kind::try_from] to create a custom kind, which may
    /// then be passed to [crate::ContextBuilder::kind].
    pub fn user() -> Self {
        Self(Cow::Borrowed("user"))
    }

    pub(crate) fn multi() -> Self {
        Self(Cow::Borrowed("multi"))
    }

    #[cfg(test)]
    // Constructs a Kind from an arbitrary string, which may result in a Kind that
    // violates the normal requirements.
    pub(crate) fn from(s: &str) -> Self {
        Kind(Cow::Owned(s.to_owned()))
    }
}

impl AsRef<str> for Kind {
    /// Returns a reference to the kind's underlying string.
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Ord for Kind {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_ref().cmp(other.as_ref())
    }
}

impl PartialOrd for Kind {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq<&str> for Kind {
    fn eq(&self, other: &&str) -> bool {
        self.as_ref() == *other
    }
}

impl PartialEq<str> for Kind {
    fn eq(&self, other: &str) -> bool {
        self.as_ref() == other
    }
}

impl Default for Kind {
    /// Kind defaults to "user".
    fn default() -> Self {
        Kind::user()
    }
}

impl TryFrom<String> for Kind {
    type Error = String;

    /// Fallibly constructs a kind from an owned string.
    /// To be a valid kind, the value cannot be empty or equal to "kind".
    /// Additionally, it must be composed entirely of ASCII alphanumeric characters, in
    /// addition to `-`, `.` and `_`.
    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "" => Err(String::from("context kind cannot be empty")),
            "kind" => Err(String::from("context kind cannot be 'kind'")),
            "multi" => Err(String::from("context kind cannot be 'multi'")),
            "user" => Ok(Kind::user()),
            k if !k
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_')) =>
            {
                Err(String::from("context kind contains disallowed characters"))
            }
            _ => Ok(Kind(Cow::Owned(value))),
        }
    }
}

impl TryFrom<&str> for Kind {
    type Error = String;

    /// Fallibly constructs a kind from a string reference.
    /// See [Kind::try_from].
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "user" => Ok(Kind::user()),
            _ => Self::try_from(value.to_owned()),
        }
    }
}

impl fmt::Display for Kind {
    /// Displays the string representation of a kind.
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_ref())
    }
}

impl From<Kind> for String {
    /// Converts a kind into its string representation.
    fn from(k: Kind) -> Self {
        k.0.into_owned()
    }
}

impl Serialize for Kind {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_ref())
    }
}

impl<'de> Deserialize<'de> for Kind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let kind = s.as_str().try_into().map_err(Error::custom)?;
        Ok(kind)
    }
}

#[cfg(test)]
pub(crate) mod proptest_generators {
    use super::Kind;
    use proptest::prelude::*;

    prop_compose! {
        pub(crate) fn any_kind_string()(
            s in "[-._a-zA-Z0-9]+".prop_filter("must not be 'kind' or 'multi'", |s| s != "kind" && s != "multi")
        ) -> String {
            s
        }
    }

    prop_compose! {
        pub(crate) fn any_kind()(s in any_kind_string()) -> Kind {
            Kind::from(s.as_str())
        }
    }
}

/// Context is a collection of attributes that can be referenced in flag evaluations and analytics
/// events. These attributes are described by one or more [Kind]s.
///
/// For example, a context might represent the user of a service, the description of an organization,
/// IoT device metadata, or any combination of those at once.
///
/// To create a context of a single kind, such as a user, you may use [crate::ContextBuilder].
///
/// To create a context with multiple kinds, use [crate::MultiContextBuilder].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "ContextVariant", into = "ContextVariant")]
pub struct Context {
    // Kind is required. For multi-contexts, Kind will always be Kind::Multi.
    pub(super) kind: Kind,
    // Contexts is only present for a multi-context.
    pub(super) contexts: Option<Vec<Context>>,
    // Name may be optionally set.
    pub(super) name: Option<String>,
    // Anonymous may be optionally set, but is false by default.
    pub(super) anonymous: bool,
    // Private attributes may be optionally set.
    pub(super) private_attributes: Option<Vec<Reference>>,
    // All Contexts have a canonical key, which is a way of uniquely representing all the
    // (kind, key) pairs in a Context.
    pub(super) canonical_key: String,
    // Attributes that aren't builtins may be optionally set.
    pub(super) attributes: HashMap<String, AttributeValue>,
    // Secondary serves as an additional key for bucketing purposes.
    // It has been deprecated by the u2c specification, and can only be set by deserializing
    // pre-Context data. Its presence is necessary for backwards-compatibility.
    pub(super) secondary: Option<String>,
    // Single contexts have keys, which serve as identifiers. For a multi-context,
    // key is an empty string.
    pub(super) key: String,
}

impl Context {
    /// Returns true if the context is a multi-context.
    pub fn is_multi(&self) -> bool {
        self.kind.is_multi()
    }

    /// Looks up the value of any attribute of the context, or a value contained within an
    /// attribute, based on the given reference.
    ///
    /// This lookup includes only attributes that are addressable in evaluations-- not metadata
    /// such as private attributes.
    ///
    /// This method implements the same behavior that the SDK uses to resolve attribute references during a flag
    /// evaluation. In a single context, the reference can represent a simple attribute name-- either a
    /// built-in one like "name" or "key", or a custom attribute that was set by methods like
    /// [crate::ContextBuilder::set_string]-- or, it can be a slash-delimited path.
    ///
    /// For a multi-context, the only supported attribute name is "kind". Use
    /// [Context::as_kind] to inspect a context for a particular [Kind] and then get its
    /// attributes.
    pub fn get_value(&self, reference: &Reference) -> Option<AttributeValue> {
        if !reference.is_valid() {
            return None;
        }

        let first_path_component = reference.component(0)?;

        if self.is_multi() {
            if reference.depth() == 1 && first_path_component == "kind" {
                return Some(AttributeValue::String(self.kind.to_string()));
            }

            warn!("Multi-contexts only support retrieving the 'kind' attribute");
            return None;
        }

        let mut attribute =
            self.get_top_level_addressable_attribute_single_kind(first_path_component)?;

        for i in 1..reference.depth() {
            let name = reference.component(i)?;
            if let AttributeValue::Object(map) = attribute {
                attribute = map.get(name).cloned()?;
            } else {
                return None;
            }
        }

        Some(attribute)
    }

    /// Returns the "key" attribute.
    ///
    /// For a single context, this value is set by the [crate::ContextBuilder::new] or
    /// [crate::ContextBuilder::key] methods.
    ///
    /// For a multi-context, there is no single key, so [Context::key] returns an empty string; use
    /// [Context::as_kind] to inspect a context for a particular kind and call [Context::key] on it.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Returns the canonical key.
    ///
    /// 1. For a single context of kind "user", the canonical key is equivalent to the key.
    /// 2. For other kinds of single contexts, the canonical key is "kind:key".
    /// 3. For a multi-context, the canonical key is the concatenation of its constituent contexts'
    /// canonical keys with `:` according to (2) (including kind "user").
    pub fn canonical_key(&self) -> &str {
        &self.canonical_key
    }

    /// Returns the "kind" attribute.
    pub fn kind(&self) -> &Kind {
        &self.kind
    }

    /// If the specified kind exists within the context, returns a reference to it.
    /// Otherwise, returns None.
    pub fn as_kind(&self, kind: &Kind) -> Option<&Context> {
        match &self.contexts {
            Some(contexts) => contexts.iter().find(|c| c.kind() == kind),
            None => self.kind.eq(kind).then(|| self),
        }
    }

    /// Returns a map of all (kind, key) pairs contained in this context.
    pub fn context_keys(&self) -> HashMap<&Kind, &str> {
        match &self.contexts {
            Some(contexts) => contexts
                .iter()
                .map(|context| (context.kind(), context.key()))
                .collect(),
            None => hashmap! { self.kind() => self.key() },
        }
    }

    /// Returns a list of all kinds represented by this context.
    pub fn kinds(&self) -> Vec<&Kind> {
        if !self.is_multi() {
            return vec![self.kind()];
        }

        match &self.contexts {
            Some(contexts) => contexts.iter().map(|context| context.kind()).collect(),
            None => Vec::new(),
        }
    }

    fn get_optional_attribute_names(&self) -> Vec<String> {
        if self.is_multi() {
            return Vec::new();
        }

        let mut names = Vec::with_capacity(self.attributes.len() + 1);
        names.extend(self.attributes.keys().cloned());

        if self.name.is_some() {
            names.push(String::from("name"));
        }

        names
    }

    pub(crate) fn bucket(
        &self,
        by_attr: &Option<Reference>,
        prefix: BucketPrefix,
        is_experiment: bool,
        context_kind: &Kind,
    ) -> Result<(f32, bool), String> {
        let reference = match (is_experiment, by_attr) {
            (true, _) | (false, None) => Reference::new("key"),
            (false, Some(reference)) => reference.clone(),
        };

        if !reference.is_valid() {
            return Err(reference.error());
        }

        match self.as_kind(context_kind) {
            Some(context) => {
                let attr_value = context.get_value(&reference);

                Ok((
                    self._bucket(attr_value.as_ref(), prefix, is_experiment)
                        .unwrap_or(0.0),
                    false,
                ))
            }
            // If the required context wasn't found, we still want the bucket to be 0, but we need
            // to show that the context was missing. This will affect the inExperiment field
            // upstream.
            _ => Ok((0.0, true)),
        }
    }

    fn _bucket(
        &self,
        value: Option<&AttributeValue>,
        prefix: BucketPrefix,
        is_experiment: bool,
    ) -> Option<f32> {
        let mut id = value?.as_bucketable()?;

        if cfg!(feature = "secondary_key_bucketing") && !is_experiment {
            if let Some(secondary) = &self.secondary {
                id.push('.');
                id.push_str(secondary);
            }
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

    fn get_top_level_addressable_attribute_single_kind(
        &self,
        name: &str,
    ) -> Option<AttributeValue> {
        match name {
            "kind" => Some(AttributeValue::String(self.kind.to_string())),
            "key" => Some(AttributeValue::String(self.key.clone())),
            "name" => self.name.clone().map(AttributeValue::String),
            "anonymous" => Some(AttributeValue::Bool(self.anonymous)),
            _ => self.attributes.get(name).map(|v| v.to_owned()),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum BucketPrefix<'a> {
    KeyAndSalt(&'a str, &'a str),
    Seed(i64),
}

impl<'a> BucketPrefix<'a> {
    pub(crate) fn write_hash(&self, hash: &mut Sha1) {
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

#[derive(Debug)]
struct PrivateAttributeLookupNode {
    reference: Option<Reference>,
    children: HashMap<String, Box<PrivateAttributeLookupNode>>,
}

/// ContextAttributes is used to handle redaction of select context properties when serializing a
/// [Context] that will be sent to LaunchDarkly.
#[derive(Debug)]
pub struct ContextAttributes {
    context: Context,
    all_attributes_private: bool,
    global_private_attributes: HashMap<String, Box<PrivateAttributeLookupNode>>,
}

impl ContextAttributes {
    /// Construct from a source context, indicating if all attributes should be private,
    /// and providing a set of attribute references that should be selectively marked private.
    pub fn from_context(
        context: Context,
        all_attributes_private: bool,
        private_attributes: HashSet<Reference>,
    ) -> Self {
        Self {
            context,
            all_attributes_private,
            global_private_attributes: Self::make_private_attribute_lookup_data(private_attributes),
        }
    }

    // This function transforms a set of [Reference]s into a data structure that allows for more
    // efficient check_global_private_attribute_refs.
    //
    // For instance, if the original references were "/name", "/address/street", and
    // "/address/city", it would produce the following map:
    //
    // "name": {
    //   attribute: Reference::new("/name"),
    // },
    // "address": {
    //   children: {
    //     "street": {
    //       attribute: Reference::new("/address/street/"),
    //     },
    //     "city": {
    //       attribute: Reference::new("/address/city/"),
    //     },
    //   },
    // }
    fn make_private_attribute_lookup_data(
        references: HashSet<Reference>,
    ) -> HashMap<String, Box<PrivateAttributeLookupNode>> {
        let mut return_value = HashMap::new();

        for reference in references.into_iter() {
            let mut parent_map = &mut return_value;
            for i in 0..reference.depth() {
                if let Some(name) = reference.component(i) {
                    if !parent_map.contains_key(name) {
                        let mut next_node = Box::new(PrivateAttributeLookupNode {
                            reference: None,
                            children: HashMap::new(),
                        });

                        if i == reference.depth() - 1 {
                            next_node.reference = Some(reference.clone());
                        }

                        parent_map.insert(name.to_owned(), next_node);
                    }

                    parent_map = &mut parent_map.get_mut(name).unwrap().children;
                }
            }
        }

        return_value
    }

    fn write_multi_context(&self) -> HashMap<String, AttributeValue> {
        let mut map: HashMap<String, AttributeValue> = HashMap::new();
        map.insert("kind".to_string(), self.context.kind().to_string().into());

        if let Some(contexts) = &self.context.contexts {
            for context in contexts.iter() {
                let context_map = self.write_single_context(context, false);
                map.insert(
                    context.kind().to_string(),
                    AttributeValue::Object(context_map),
                );
            }
        }

        map
    }

    fn write_single_context(
        &self,
        context: &Context,
        include_kind: bool,
    ) -> HashMap<String, AttributeValue> {
        let mut map: HashMap<String, AttributeValue> = HashMap::new();

        if include_kind {
            map.insert("kind".into(), context.kind().to_string().into());
        }

        map.insert(
            "key".to_string(),
            AttributeValue::String(context.key().to_owned()),
        );

        let optional_attribute_names = context.get_optional_attribute_names();
        let mut redacted_attributes = Vec::<String>::with_capacity(20);

        for key in optional_attribute_names.iter() {
            let reference = Reference::new(key);
            if let Some(value) = context.get_value(&reference) {
                // If all_attributes_private is true, then there's no complex filtering or
                // recursing to be done: all of these values are by definition private, so just add
                // their names to the redacted list.
                if self.all_attributes_private {
                    redacted_attributes.push(String::from(reference));
                    continue;
                }

                let path = Vec::with_capacity(10);
                self.write_filter_attribute(
                    context,
                    &mut map,
                    path,
                    key,
                    value,
                    &mut redacted_attributes,
                )
            }
        }

        if context.anonymous {
            map.insert("anonymous".to_string(), true.into());
        }

        if context.secondary.is_some() || !redacted_attributes.is_empty() {
            let mut meta: HashMap<String, AttributeValue> = HashMap::new();
            if let Some(secondary) = &context.secondary {
                meta.insert(
                    "secondary".to_string(),
                    AttributeValue::String(secondary.to_string()),
                );
            }

            if !redacted_attributes.is_empty() {
                meta.insert(
                    "redactedAttributes".to_string(),
                    AttributeValue::Array(
                        redacted_attributes
                            .into_iter()
                            .map(AttributeValue::String)
                            .collect(),
                    ),
                );
            }

            map.insert("_meta".to_string(), AttributeValue::Object(meta));
        }

        map
    }

    // Checks whether a given value should be considered private, and then either writes the
    // attribute to the output HashMap if it is *not* private, or adds the corresponding attribute
    // reference to the redacted_attributes list if it is private.
    //
    // The parent_path parameter indicates where we are in the context data structure. If it is
    // empty, we are at the top level and "key" is an attribute name. If it is not empty, we are
    // recursing into the properties of an attribute value that is a JSON object: for instance, if
    // parent_path is ["billing", "address"] and key is "street", then the top-level attribute is
    // "billing" and has a value in the form {"address": {"street": ...}} and we are now deciding
    // whether to write the "street" property. See maybe_redact for the logic involved in that
    // decision.
    //
    // If all_attributes_private is true, this method is never called.
    fn write_filter_attribute(
        &self,
        context: &Context,
        map: &mut HashMap<String, AttributeValue>,
        parent_path: Vec<String>,
        key: &str,
        value: AttributeValue,
        redacted_attributes: &mut Vec<String>,
    ) {
        let mut path = parent_path;
        path.push(key.to_string());

        let (is_redacted, nested_properties_are_redacted) =
            self.maybe_redact(context, &path, &value, redacted_attributes);

        // If the value is an object, then there are three possible outcomes:
        //
        // 1. this value is completely redacted, so drop it and do not recurse;
        // 2. the value is not redacted, and and neither are any subproperties within it, so output
        //    the whole thing as-is;
        // 3. the value itself is not redacted, but some subproperties within it are, so we'll need
        //    to recurse through it and filter as we go.
        match value {
            AttributeValue::Object(_) if is_redacted => (), // outcome 1
            AttributeValue::Object(ref object_map) => {
                // outcome 2
                if !nested_properties_are_redacted {
                    map.insert(key.to_string(), value.clone());
                    return;
                }

                // outcome 3
                let mut sub_map = HashMap::new();
                for (k, v) in object_map.iter() {
                    self.write_filter_attribute(
                        context,
                        &mut sub_map,
                        path.clone(),
                        k,
                        v.clone(),
                        redacted_attributes,
                    );
                }
                map.insert(key.to_string(), AttributeValue::Object(sub_map));
            }
            _ if !is_redacted => {
                map.insert(key.to_string(), value);
            }
            _ => (),
        };
    }

    // Called by write_filter_attribute to decide whether or not a given value (or, possibly,
    // properties within it) should be considered private, based on the private attribute
    // references.
    //
    // If the value should be private, then the first return value is true, and also the attribute
    // reference is added to redacted_attributes.
    //
    // The second return value indicates whether there are any private attribute references
    // designating properties *within* this value. That is, if parent_path is ["address"], and the
    // configuration says that "/address/street" is private, then the second return value will be
    // true, which tells us that we can't just dump the value of the "address" object directly into
    // the output but will need to filter its properties.
    //
    // Note that even though a Reference can contain numeric path components to represent an array
    // element lookup, for the purposes of flag evaluations (like "/animals/0" which conceptually
    // represents context.animals[0]), those will not work as private attribute references since
    // we do not recurse to redact anything within an array value. A reference like "/animals/0"
    // would only work if context.animals were an object with a property named "0".
    //
    // If all_attributes_private is true, this method is never called.
    fn maybe_redact(
        &self,
        context: &Context,
        parent_path: &[String],
        value: &AttributeValue,
        redacted_attributes: &mut Vec<String>,
    ) -> (bool, bool) {
        let (redacted_attr_ref, mut nested_properties_are_redacted) =
            self.check_global_private_attribute_refs(parent_path);

        if let Some(redacted_attr_ref) = redacted_attr_ref {
            redacted_attributes.push(String::from(redacted_attr_ref));
            return (true, false);
        }

        let should_check_for_nested_properties = matches!(value, AttributeValue::Object(..));

        if let Some(private_attributes) = &context.private_attributes {
            for private_attribute in private_attributes.iter() {
                let depth = private_attribute.depth();
                if depth < parent_path.len() {
                    // If the attribute reference is shorter than the current path, then it can't
                    // possibly be a match, because if it had matched the first part of our path,
                    // we wouldn't have recursed this far.
                    continue;
                }

                if !should_check_for_nested_properties && depth > parent_path.len() {
                    continue;
                }

                let mut has_match = true;
                for (i, parent_part) in parent_path.iter().enumerate() {
                    match private_attribute.component(i) {
                        None => break,
                        Some(name) if name != parent_part => {
                            has_match = false;
                            break;
                        }
                        _ => continue,
                    };
                }

                if has_match {
                    if depth == parent_path.len() {
                        redacted_attributes.push(private_attribute.to_string());
                        return (true, false);
                    }
                    nested_properties_are_redacted = true;
                }
            }
        }

        (false, nested_properties_are_redacted)
    }

    // Checks whether the given attribute or subproperty matches any Reference that was designated
    // as private in the SDK options.
    //
    // If parent_path has just one element, it is the name of a top-level attribute. If it has
    // multiple elements, it is a path to a property within a custom object attribute: for
    // instance, if you represented the overall context as a JSON object, the parent_path
    // ["billing", "address", "street"] would refer to the street property within something like
    // {"billing": {"address": {"street": "x"}}}.
    //
    // The first return value is None if the attribute does not need to be redacted; otherwise it
    // is the specific attribute reference that was matched.
    //
    // The second return value is true if and only if there's at least one configured private
    // attribute reference for *children* of parent_path (and there is not one for parent_path
    // itself, since if there was, we would not bother recursing to write the children). See
    // comments on write_filter_attribute.
    fn check_global_private_attribute_refs(
        &self,
        parent_path: &[String],
    ) -> (Option<Reference>, bool) {
        let mut lookup = &self.global_private_attributes;

        if self.global_private_attributes.is_empty() {
            return (None, false);
        }

        for (index, path) in parent_path.iter().enumerate() {
            let next_node = match lookup.get(path.as_str()) {
                None => break,
                Some(v) => v,
            };

            if index == parent_path.len() - 1 {
                let var_name = (next_node.reference.clone(), next_node.reference.is_none());
                return var_name;
            } else if !next_node.children.is_empty() {
                lookup = &next_node.children;
            }
        }

        (None, false)
    }
}

impl<'dispatcher> ser::Serialize for ContextAttributes {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: ser::Serializer,
    {
        let mut serialize_map = serializer.serialize_map(None)?;

        let map = match self.context.is_multi() {
            true => self.write_multi_context(),
            false => self.write_single_context(&self.context, true),
        };

        for (k, v) in map.iter().sorted_by_key(|p| p.0) {
            serialize_map.serialize_entry(k, v)?;
        }

        serialize_map.end()
    }
}

#[cfg(test)]
mod tests {
    use super::proptest_generators::*;
    use crate::{AttributeValue, ContextBuilder, MultiContextBuilder, Reference};
    use maplit::hashmap;
    use proptest::proptest;
    use test_case::test_case;

    use super::Kind;

    proptest! {
        #[test]
        fn all_generated_kinds_are_valid(kind in any_kind()) {
            let maybe_kind = Kind::try_from(kind.as_ref());
            assert!(maybe_kind.is_ok());
        }
    }

    #[test_case("kind"; "Cannot set kind as kind")]
    #[test_case("multi"; "Cannot set kind as multi")]
    #[test_case("🦀"; "Cannot set kind as invalid character")]
    #[test_case(" "; "Cannot set kind as only whitespace")]
    fn invalid_kinds(kind: &str) {
        assert!(Kind::try_from(kind).is_err());
    }

    #[test_case(Kind::user(), true)]
    #[test_case(Kind::from("user"), true)]
    #[test_case(Kind::multi(), false)]
    #[test_case(Kind::from("foo"), false)]
    fn is_user(kind: Kind, is_user: bool) {
        assert_eq!(kind.is_user(), is_user);
    }

    #[test_case(Kind::multi(), true)]
    #[test_case(Kind::from("multi"), true)]
    #[test_case(Kind::user(), false)]
    #[test_case(Kind::from("foo"), false)]
    fn is_multi(kind: Kind, is_multi: bool) {
        assert_eq!(kind.is_multi(), is_multi);
    }

    #[test]
    fn kind_sorts_based_on_string() {
        let mut kinds = vec![
            Kind::user(),
            Kind::multi(),
            Kind::from("n"),
            Kind::from("v"),
            Kind::from("l"),
        ];
        kinds.sort();
        assert_eq!(
            kinds,
            vec![
                Kind::from("l"),
                Kind::multi(),
                Kind::from("n"),
                Kind::user(),
                Kind::from("v"),
            ]
        );
    }

    proptest! {
        #[test]
        fn kind_comparison_identity(kind in any_kind()) {
            assert_eq!(kind, kind);
        }
    }

    proptest! {
        #[test]
        fn kind_comparison_identity_str(kind in any_kind()) {
            assert_eq!(kind, kind.as_ref());
            assert_eq!(&kind, kind.as_ref());
        }
    }

    proptest! {
        #[test]
        fn kind_comparison_different(a in any_kind(), b in any_kind()) {
            if a.0 != b.0 {
                assert_ne!(a, b);
            }
        }
    }

    proptest! {
        #[test]
        fn kind_serialize(kind in any_kind()) {
            assert_eq!(format!("\"{}\"", kind.0), serde_json::to_string(&kind).unwrap());
        }
    }

    proptest! {
        #[test]
        fn kind_deserialize(kind_str in any_kind_string()) {
            let json_str = format!("\"{}\"", &kind_str);
            let kind: Result<Kind, _> = serde_json::from_str(&json_str);
            assert!(kind.is_ok());
        }
    }

    // Since "multi" is reserved as the signifier for multi-contexts,
    // it cannot be constructed directly.
    #[test]
    fn cannot_deserialize_multi_kind() {
        let maybe_kind: Result<Kind, _> = serde_json::from_str("\"multi\"");
        assert!(maybe_kind.is_err());
    }

    // Basic simple attribute retrievals
    #[test_case("kind", Some(AttributeValue::String("org".to_string())))]
    #[test_case("key", Some(AttributeValue::String("my-key".to_string())))]
    #[test_case("name", Some(AttributeValue::String("my-name".to_string())))]
    #[test_case("anonymous", Some(AttributeValue::Bool(true)))]
    #[test_case("attr", Some(AttributeValue::String("my-attr".to_string())))]
    #[test_case("/starts-with-slash", Some(AttributeValue::String("love that prefix".to_string())))]
    #[test_case("/crazy~0name", Some(AttributeValue::String("still works".to_string())))]
    #[test_case("/other", None)]
    // Invalid reference retrieval
    #[test_case("/", None; "Single slash")]
    #[test_case("", None; "Empty reference")]
    #[test_case("/a//b", None; "Double slash")]
    // Hidden meta attributes
    #[test_case("privateAttributes", None)]
    #[test_case("secondary", None)]
    // Can index objects
    #[test_case("/my-map/array", Some(AttributeValue::Array(vec![AttributeValue::String("first".to_string()), AttributeValue::String("second".to_string())])))]
    #[test_case("/my-map/1", Some(AttributeValue::Bool(true)))]
    #[test_case("/my-map/missing", None)]
    #[test_case("/starts-with-slash/1", None; "handles providing an index to a non-array value")]
    fn context_can_get_value(input: &str, value: Option<AttributeValue>) {
        let mut builder = ContextBuilder::new("my-key");

        let array = vec![
            AttributeValue::String("first".to_string()),
            AttributeValue::String("second".to_string()),
        ];
        let map = hashmap! {
            "array".to_string() => AttributeValue::Array(array),
            "1".to_string() => AttributeValue::Bool(true)
        };

        let context = builder
            .kind("org".to_string())
            .name("my-name")
            .anonymous(true)
            .secondary("my-secondary")
            .set_string("attr", "my-attr")
            .set_string("starts-with-slash", "love that prefix")
            .set_string("crazy~name", "still works")
            .set_value("my-map", AttributeValue::Object(map))
            .add_private_attribute("attr")
            .build()
            .expect("Failed to build context");

        assert_eq!(context.get_value(&Reference::new(input)), value);
    }

    #[test_case("kind", Some(AttributeValue::String("multi".to_string())))]
    #[test_case("key", None)]
    #[test_case("name", None)]
    #[test_case("anonymous", None)]
    #[test_case("attr", None)]
    fn multi_context_get_value(input: &str, value: Option<AttributeValue>) {
        let mut multi_builder = MultiContextBuilder::new();
        let mut builder = ContextBuilder::new("user");

        multi_builder.add_context(builder.build().expect("Failed to create context"));

        builder
            .key("org")
            .kind("org".to_string())
            .name("my-name")
            .anonymous(true)
            .set_string("attr", "my-attr");
        multi_builder.add_context(builder.build().expect("Failed to create context"));

        let context = multi_builder.build().expect("Failed to create context");

        assert_eq!(context.get_value(&Reference::new(input)), value);
    }

    #[test]
    fn can_retrieve_context_from_multi_context() {
        let user_context = ContextBuilder::new("user").build().unwrap();
        let org_context = ContextBuilder::new("org").kind("org").build().unwrap();

        assert!(org_context.as_kind(&Kind::user()).is_none());

        let multi_context = MultiContextBuilder::new()
            .add_context(user_context)
            .add_context(org_context)
            .build()
            .unwrap();

        assert!(multi_context
            .as_kind(&Kind::user())
            .unwrap()
            .kind()
            .is_user());

        assert_eq!(
            "org",
            multi_context
                .as_kind(&Kind::from("org"))
                .unwrap()
                .kind()
                .as_ref()
        );

        assert!(multi_context.as_kind(&Kind::from("custom")).is_none());
    }
}
