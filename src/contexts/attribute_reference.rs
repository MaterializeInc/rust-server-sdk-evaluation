use serde::{Deserialize, Serialize, Serializer};
use std::fmt::Display;

#[derive(Clone, Hash, PartialEq, Eq, Debug, Serialize)]
enum Error {
    Empty,
    InvalidEscapeSequence,
    DoubleOrTrailingSlash,
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Error::Empty => write!(f, "Reference cannot be empty"),
            Error::InvalidEscapeSequence => write!(f, "Reference contains invalid escape sequence"),
            Error::DoubleOrTrailingSlash => {
                write!(f, "Reference contains double or trailing slash")
            }
        }
    }
}

/// Represents an attribute name or path expression identifying a value within a [crate::Context].
///
/// This can be used to retrieve a value with [crate::Context::get_value], or to identify an attribute or
/// nested value that should be considered private with
/// [crate::ContextBuilder::add_private_attribute] (the SDK configuration can also have a list of
/// private attribute references).
///
/// This is represented as a separate type, rather than just a string, so that validation and parsing can
/// be done ahead of time if an attribute reference will be used repeatedly later (such as in flag
/// evaluations).
///
/// If the string starts with '/', then this is treated as a slash-delimited path reference where the
/// first component is the name of an attribute, and subsequent components are the names of nested JSON
/// object properties. In this syntax, the escape sequences "~0" and "~1" represent '~' and '/'
/// respectively within a path component.
///
/// If the string does not start with '/', then it is treated as the literal name of an attribute.
///
/// # Example
/// ```
/// # use crate::launchdarkly_server_sdk_evaluation::{ContextBuilder, Context, Reference, AttributeValue};
/// # use serde_json::json;
/// # let context: Context = serde_json::from_value(json!(
/// // Given the following JSON representation of a context:
/// {
///   "kind": "user",
///   "key": "123",
///   "name": "xyz",
///   "address": {
///     "street": "99 Main St.",
///     "city": "Westview"
///   },
///   "a/b": "ok"
/// }
/// # )).unwrap();
///
/// assert_eq!(context.get_value(&Reference::new("name")),
///     Some(AttributeValue::String("xyz".to_owned())));
/// assert_eq!(context.get_value(&Reference::new("/address/street")),
///     Some(AttributeValue::String("99 Main St.".to_owned())));
/// assert_eq!(context.get_value(&Reference::new("a/b")),
///     Some(AttributeValue::String("ok".to_owned())));
/// assert_eq!(context.get_value(&Reference::new("/a~1b")),
///     Some(AttributeValue::String("ok".to_owned())));
/// ```
#[derive(Clone, Hash, PartialEq, Eq, Debug)]
pub struct Reference {
    variant: Variant,
    input: String,
}

#[derive(Clone, Hash, PartialEq, Eq, Debug)]
enum Variant {
    /// Represents a plain, top-level attribute name; does not start with a '/'.
    PlainName,
    /// Represents an attribute pointer; starts with a '/'.
    Pointer(Vec<String>),
    /// Represents an invalid input string.
    Error(Error),
}

impl Serialize for Reference {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.input)
    }
}

impl<'de> Deserialize<'de> for Reference {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(Reference::new(s))
    }
}

impl Reference {
    /// Construct a new context attribute reference.
    ///
    /// This constructor always returns a reference that preserves the original string, even if
    /// validation fails, so that serializing the reference to JSON will produce the original
    /// string.
    pub fn new<S: AsRef<str>>(value: S) -> Self {
        let value = value.as_ref();

        if value.is_empty() || value == "/" {
            return Self {
                variant: Variant::Error(Error::Empty),
                input: value.to_owned(),
            };
        }

        if !value.starts_with('/') {
            return Self {
                variant: Variant::PlainName,
                input: value.to_owned(),
            };
        }

        let component_result = value[1..]
            .split('/')
            .into_iter()
            .map(|part| {
                if part.is_empty() {
                    return Err(Error::DoubleOrTrailingSlash);
                }
                Reference::unescape_path(part)
            })
            .collect::<Result<Vec<String>, Error>>();

        match component_result {
            Ok(components) => Self {
                variant: Variant::Pointer(components),
                input: value.to_owned(),
            },
            Err(e) => Self {
                variant: Variant::Error(e),
                input: value.to_owned(),
            },
        }
    }

    /// Returns true if the reference is valid.
    pub fn is_valid(&self) -> bool {
        !matches!(&self.variant, Variant::Error(_))
    }

    /// If the reference is invalid, this method returns an error description; otherwise, it
    /// returns an empty string.
    pub fn error(&self) -> String {
        match &self.variant {
            Variant::Error(e) => e.to_string(),
            _ => "".to_owned(),
        }
    }

    /// Returns the number of path components in the reference.
    ///
    /// For a simple attribute reference such as "name" with no leading slash, this returns 1.
    ///
    /// For an attribute reference with a leading slash, it is the number of slash-delimited path
    /// components after the initial slash.
    /// # Example
    /// ```
    /// # use crate::launchdarkly_server_sdk_evaluation::Reference;
    /// assert_eq!(Reference::new("a").depth(), 1);
    /// assert_eq!(Reference::new("/a/b").depth(), 2);
    /// ```
    pub fn depth(&self) -> usize {
        match &self.variant {
            Variant::Pointer(components) => components.len(),
            Variant::PlainName => 1,
            _ => 0,
        }
    }

    /// Retrieves a single path component from the attribute reference.
    ///
    /// Returns the attribute name for a simple attribute reference such as "name" with no leading slash, if index is zero.
    ///
    /// Returns the specified path component if index is less than [Reference::depth], and the reference begins with a slash.
    ///
    /// If index is out of range, it returns None.
    ///
    /// # Examples
    /// ```
    /// # use launchdarkly_server_sdk_evaluation::Reference;
    /// assert_eq!(Reference::new("a").component(0), Some("a"));
    /// assert_eq!(Reference::new("/a/b").component(1), Some("b"));
    /// assert_eq!(Reference::new("/a/b").component(2), None);
    /// ```
    pub fn component(&self, index: usize) -> Option<&str> {
        match (&self.variant, index) {
            (Variant::Pointer(components), _) => components.get(index).map(|c| c.as_str()),
            (Variant::PlainName, 0) => Some(&self.input),
            _ => None,
        }
    }

    // Checks if the Reference resolves to a Context's 'kind' attribute.
    pub(crate) fn is_kind(&self) -> bool {
        matches!((self.depth(), self.component(0)), (1, Some(comp)) if comp == "kind")
    }

    fn unescape_path(path: &str) -> Result<String, Error> {
        // If there are no tildes then there's definitely nothing to do
        if !path.contains('~') {
            return Ok(path.to_string());
        }

        let mut out = String::new();

        let mut iter = path.chars().peekable();
        while let Some(c) = iter.next() {
            if c != '~' {
                out.push(c);
                continue;
            }
            if iter.peek().is_none() {
                return Err(Error::InvalidEscapeSequence);
            }

            let unescaped = match iter.next().unwrap() {
                '0' => '~',
                '1' => '/',
                _ => return Err(Error::InvalidEscapeSequence),
            };
            out.push(unescaped);
        }

        Ok(out)
    }
}

impl Default for Reference {
    /// A default [Reference] is empty and invalid.
    fn default() -> Self {
        Reference::new("")
    }
}

/// Displays the input string used to construct the [Reference].
impl Display for Reference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        write!(f, "{}", self.input)
    }
}

impl<S> From<S> for Reference
where
    S: AsRef<str>,
{
    fn from(reference: S) -> Self {
        Reference::new(reference)
    }
}

impl From<Reference> for String {
    fn from(r: Reference) -> Self {
        r.input
    }
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(transparent)]
/// Represents an attribute name, found in pre-Context data.
/// AttributeNames are incapable of referring to nested values, and instead only
/// refer to top-level attributes.  
pub(crate) struct AttributeName(String);

impl AttributeName {
    /// Constructs an AttributeName, which can be converted into an equivalent [Reference].
    #[cfg(test)]
    pub(crate) fn new(s: String) -> Self {
        Self(s)
    }
}

impl Default for AttributeName {
    fn default() -> Self {
        Self("".to_owned())
    }
}

impl From<AttributeName> for Reference {
    /// AttributeNames are converted into References based on the presence or
    /// absence of a leading '/'.
    ///
    /// Although References are able to represent plain, top-level attribute
    /// names, they cannot represent those that begin with a leading '/' because that signifies
    /// the pointer syntax.
    ///
    /// Therefore, if the first character is a '/' the string must be escaped.
    ///
    /// This results in the equivalent [Reference] representation of that [AttributeName].
    ///
    /// Note that References constructed from an AttributeName will serialize to the
    /// string passed into the Reference constructor, not the original AttributeName. This
    /// is desirable since data should be "upgraded" into the new format as it is encountered.
    fn from(name: AttributeName) -> Self {
        if !name.0.starts_with('/') {
            return Self::new(name.0);
        }
        let mut escaped = name.0.replace('~', "~0").replace('/', "~1");
        escaped.insert(0, '/');
        Self::new(escaped)
    }
}

#[cfg(test)]
pub(crate) mod proptest_generators {
    use super::{AttributeName, Reference};
    use proptest::prelude::*;

    // This regular expression is meant to match our spec for an acceptable attribute string,
    // both those representing attribute references, and those representing literal attribute
    // names.
    // A. Plain attribute names are handled by the first alternative (not beginning with '/')
    // B. Attribute references are handled by the second alternative.
    //    1) Starts with a slash
    //    2) Followed by any character that isn't a / or ~ (they must be escaped with ~1 and ~0)
    //    3) Or, an occurrence of ~1 or ~0
    //    4) At least one of 2) or 3) is required.
    //    The path component can repeat one or more times.
    prop_compose! {
        // Generate any string that could represent a valid reference, either using
        // JSON-pointer-like syntax, or plain attribute name. Will not return an empty string.
        pub(crate) fn any_valid_ref_string()(s in "([^/].*|(/([^/~]|~[01])+)+)") -> String {
            s
        }

    }

    prop_compose! {
         pub(crate) fn any_valid_plain_name()(s in "([^/].*)") -> String {
            s
         }
    }

    prop_compose! {
         pub(crate) fn any_attribute_name()(s in any_valid_ref_string()) -> AttributeName {
            AttributeName::new(s)
         }
    }

    prop_compose! {
        // Generate any valid reference.
        pub(crate) fn any_valid_ref()(s in any_valid_ref_string()) -> Reference {
            Reference::new(s)
        }
    }

    prop_compose! {
        // Generate any reference, invalid or not. May generate empty strings.
        pub(crate) fn any_ref()(s in any::<String>()) -> Reference {
            Reference::new(s)
        }
    }

    prop_compose! {
        pub(crate) fn any_valid_ref_transformed_from_attribute_name()(s in any_valid_ref_string()) -> Reference {
            Reference::from(AttributeName::new(s))
        }
    }

    prop_compose! {
        // Generate any literal reference, valid or not. May generate empty strings.
        pub(crate) fn any_ref_transformed_from_attribute_name()(s in any::<String>()) -> Reference {
            Reference::from(AttributeName::new(s))
        }
    }

    prop_compose! {
        pub(crate) fn any_valid_plain_ref()(s in any_valid_plain_name()) -> Reference {
            Reference::new(s)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AttributeName, Error, Reference};
    use crate::proptest_generators::*;
    use proptest::prelude::*;
    use test_case::test_case;

    proptest! {
        #[test]
        fn regex_creates_valid_references(reference in any_valid_ref()) {
            prop_assert!(reference.is_valid());
        }
    }

    proptest! {
        // Although this should be a subset of the previous test, it's still useful to
        // assert that it obeys the property of generating valid references on its own.
        #[test]
        fn regex_creates_valid_plain_references(reference in any_valid_plain_ref()) {
            prop_assert!(reference.is_valid());
        }
    }

    proptest! {
        #[test]
        fn plain_references_have_single_component(reference in any_valid_plain_ref()) {
            prop_assert_eq!(reference.depth(), 1);
        }
    }

    proptest! {
        #[test]
        fn attribute_names_are_valid_references(reference in any_valid_ref_transformed_from_attribute_name()) {
            prop_assert!(reference.is_valid());
            prop_assert_eq!(reference.depth(), 1);
        }
    }

    proptest! {
        #[test]
        fn attribute_name_references_have_single_component(reference in any_valid_ref_transformed_from_attribute_name()) {
            prop_assert_eq!(reference.depth(), 1);
            let component = reference.component(0);
            prop_assert!(component.is_some(), "component 0 should exist");
        }
    }

    proptest! {
        #[test]
        fn raw_returns_input_unmodified(s in any::<String>()) {
            let a = Reference::new(s.clone());
            prop_assert_eq!(a.to_string(), s);
        }
    }

    #[test]
    fn default_reference_is_invalid() {
        assert!(!Reference::default().is_valid());
    }

    #[test_case("", Error::Empty; "Empty reference")]
    #[test_case("/", Error::Empty; "Single slash")]
    #[test_case("//", Error::DoubleOrTrailingSlash; "Double slash")]
    #[test_case("/a//b", Error::DoubleOrTrailingSlash; "Double slash in middle")]
    #[test_case("/a/b/", Error::DoubleOrTrailingSlash; "Trailing slash")]
    #[test_case("/~3", Error::InvalidEscapeSequence; "Tilde must be followed by 0 or 1 only")]
    #[test_case("/testing~something", Error::InvalidEscapeSequence; "Tilde cannot be alone")]
    #[test_case("/m~~0", Error::InvalidEscapeSequence; "Extra tilde before valid escape")]
    #[test_case("/a~", Error::InvalidEscapeSequence; "Tilde cannot be followed by nothing")]
    fn invalid_references(input: &str, error: Error) {
        let reference = Reference::new(input);
        assert!(!reference.is_valid());
        assert_eq!(error.to_string(), reference.error());
    }

    #[test_case("key")]
    #[test_case("kind")]
    #[test_case("name")]
    #[test_case("name/with/slashes")]
    #[test_case("name~0~1with-what-looks-like-escape-sequences")]
    fn plain_reference_syntax(input: &str) {
        let reference = Reference::new(input);
        assert!(reference.is_valid());
        assert_eq!(input, reference.to_string());
        assert_eq!(
            input,
            reference
                .component(0)
                .expect("Failed to get first component")
        );
        assert_eq!(1, reference.depth());
    }

    #[test_case("/key", "key")]
    #[test_case("/kind", "kind")]
    #[test_case("/name", "name")]
    #[test_case("/custom", "custom")]
    fn pointer_syntax(input: &str, path: &str) {
        let reference = Reference::new(input);
        assert!(reference.is_valid());
        assert_eq!(input, reference.to_string());
        assert_eq!(
            path,
            reference
                .component(0)
                .expect("Failed to get first component")
        );
        assert_eq!(1, reference.depth())
    }

    #[test_case("/a/b", 2, 0, "a")]
    #[test_case("/a/b", 2, 1, "b")]
    #[test_case("/a~1b/c", 2, 0, "a/b")]
    #[test_case("/a~1b/c", 2, 1, "c")]
    #[test_case("/a/10/20/30x", 4, 1, "10")]
    #[test_case("/a/10/20/30x", 4, 2, "20")]
    #[test_case("/a/10/20/30x", 4, 3, "30x")]
    fn handles_subcomponents(input: &str, len: usize, index: usize, expected_name: &str) {
        let reference = Reference::new(input);
        assert!(reference.is_valid());
        assert_eq!(input, reference.input);
        assert_eq!(len, reference.depth());
        assert_eq!(expected_name, reference.component(index).unwrap());
    }

    #[test]
    fn can_handle_invalid_index_requests() {
        let reference = Reference::new("/a/b/c");
        assert!(reference.is_valid());
        assert!(reference.component(0).is_some());
        assert!(reference.component(1).is_some());
        assert!(reference.component(2).is_some());
        assert!(reference.component(3).is_none());
    }

    #[test_case("/a/b", "/~1a~1b")]
    #[test_case("a", "a")]
    #[test_case("a~1b", "a~1b")]
    #[test_case("/a~1b", "/~1a~01b")]
    #[test_case("/a~0b", "/~1a~00b")]
    #[test_case("", "")]
    #[test_case("/", "/~1")]
    fn attribute_name_equality(name: &str, reference: &str) {
        let as_name = AttributeName::new(name.to_owned());
        let reference = Reference::new(reference);
        assert_eq!(Reference::from(as_name), reference);
    }

    #[test]
    fn is_kind() {
        assert!(Reference::new("/kind").is_kind());
        assert!(Reference::new("kind").is_kind());
        assert!(Reference::from(AttributeName::new("kind".to_owned())).is_kind());

        assert!(!Reference::from(AttributeName::new("/kind".to_owned())).is_kind());
        assert!(!Reference::new("foo").is_kind());
    }
}
