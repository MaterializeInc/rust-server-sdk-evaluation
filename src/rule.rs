use crate::attribute_value::AttributeValue;
use crate::contexts::attribute_reference::AttributeName;
use crate::contexts::context::Kind;
use crate::store::Store;
use crate::variation::VariationOrRollout;
use crate::{util, Context, EvaluationStack, Reference};
use chrono::{self, Utc};
use log::{error, warn};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use util::is_false;

/// Clause describes an individual clause within a [crate::FlagRule] or `SegmentRule`.
// Clause is deserialized via a helper, IntermediateClause, because of semantic ambiguity
// of the attribute Reference field.
//
// Clause implements Serialize directly without a helper because References can serialize
// themselves without any ambiguity.
#[skip_serializing_none]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", from = "IntermediateClause")]
pub struct Clause {
    // Kind associated with this clause.
    context_kind: Kind,
    // Which context attribute to test. If op does not require an attribute,
    // then the input may be an empty string, which will construct an invalid
    // reference.
    attribute: Reference,
    // True if the result of the test should be negated.
    // Skip serializing if false because it is optional in the JSON model.
    #[serde(skip_serializing_if = "is_false")]
    negate: bool,
    // The test operation.
    op: Op,
    // The values to test against.
    values: Vec<AttributeValue>,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct ClauseWithKind {
    context_kind: Kind,
    attribute: Reference,
    #[serde(default)]
    negate: bool,
    op: Op,
    values: Vec<AttributeValue>,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct ClauseWithoutKind {
    attribute: AttributeName,
    #[serde(default)]
    negate: bool,
    op: Op,
    values: Vec<AttributeValue>,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(untagged)]
enum IntermediateClause {
    // ClauseWithKind must be listed first in the enum because otherwise ClauseWithoutKind
    // could match the input (by ignoring/discarding the context_kind field).
    ContextAware(ClauseWithKind),
    ContextOblivious(ClauseWithoutKind),
}

impl From<IntermediateClause> for Clause {
    fn from(ic: IntermediateClause) -> Self {
        match ic {
            IntermediateClause::ContextAware(fields) => Self {
                context_kind: fields.context_kind,
                attribute: fields.attribute,
                negate: fields.negate,
                op: fields.op,
                values: fields.values,
            },
            IntermediateClause::ContextOblivious(fields) => Self {
                context_kind: Kind::default(),
                attribute: Reference::from(fields.attribute),
                negate: fields.negate,
                op: fields.op,
                values: fields.values,
            },
        }
    }
}

#[cfg(test)]
pub(crate) mod proptest_generators {
    use super::Clause;
    use crate::contexts::attribute_reference::proptest_generators::*;
    use crate::contexts::context::proptest_generators::*;
    use crate::rule::Op;
    use crate::AttributeValue;
    use proptest::{collection::vec, prelude::*};

    prop_compose! {
        // Generate arbitrary clauses. The clauses op will always be fixed to Op::In,
        // and the values array will contain between 0-5 AtrributeValue::Bool elements.
        pub(crate) fn any_clause()(
            kind in any_kind(),
            // reference is any_ref(), rather than any_valid_ref(), because we also want
            // coverage of invalid references.
            reference in any_ref(),
            negate in any::<bool>(),
            values in vec(any::<bool>(), 0..5),
            op in any::<Op>()
        ) -> Clause {
            Clause {
                context_kind: kind,
                attribute: reference,
                negate,
                op,
                values: values.iter().map(|&b| AttributeValue::from(b)).collect()
            }
        }
    }
}

/// FlagRule describes a single rule within a feature flag.
///
/// A rule consists of a set of ANDed matching conditions ([Clause]) for a context, along with either a
/// fixed variation or a set of rollout percentages to use if the context matches all of the clauses.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlagRule {
    /// A randomized identifier assigned to each rule when it is created.
    ///
    /// This is used to populate the id property of [crate::Reason]
    #[serde(default)]
    pub id: String,
    clauses: Vec<Clause>,

    /// Defines what variation to return if the context matches this rule.
    #[serde(flatten)]
    pub variation_or_rollout: VariationOrRollout,

    /// Used internally by the SDK analytics event system.
    ///
    /// This field is true if the current LaunchDarkly account has experimentation enabled, has
    /// associated this flag with an experiment, and has enabled this rule for the experiment. This
    /// tells the SDK to send full event data for any evaluation that matches this rule.
    ///
    /// The launchdarkly-server-sdk-evaluation package does not implement that behavior; it is only
    /// in the data model for use by the SDK.
    pub track_events: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[cfg_attr(test, derive(proptest_derive::Arbitrary))]
#[serde(rename_all = "camelCase")]
enum Op {
    In,
    StartsWith,
    EndsWith,
    Contains,
    Matches,
    LessThan,
    LessThanOrEqual,
    GreaterThan,
    GreaterThanOrEqual,
    Before,
    After,
    SegmentMatch,
    SemVerEqual,
    SemVerGreaterThan,
    SemVerLessThan,
    #[serde(other)]
    Unknown,
}

impl Clause {
    pub(crate) fn matches(
        &self,
        context: &Context,
        store: &dyn Store,
        evaluation_stack: &mut EvaluationStack,
    ) -> Result<bool, String> {
        if let Op::SegmentMatch = self.op {
            self.matches_segment(context, store, evaluation_stack)
        } else {
            self.matches_non_segment(context)
        }
    }

    fn maybe_negate(&self, v: bool) -> bool {
        if self.negate {
            !v
        } else {
            v
        }
    }

    pub(crate) fn matches_segment(
        &self,
        context: &Context,
        store: &dyn Store,
        evaluation_stack: &mut EvaluationStack,
    ) -> Result<bool, String> {
        for value in self.values.iter() {
            if let Some(segment_key) = value.as_str() {
                if let Some(segment) = store.segment(segment_key) {
                    let matches = segment.contains(context, store, evaluation_stack)?;
                    if matches {
                        return Ok(self.maybe_negate(true));
                    }
                }
            }
        }

        Ok(self.maybe_negate(false))
    }

    pub(crate) fn matches_non_segment(&self, context: &Context) -> Result<bool, String> {
        if !self.attribute.is_valid() {
            return Err(self.attribute.error());
        }

        if self.attribute.is_kind() {
            for clause_value in &self.values {
                for kind in context.kinds().iter() {
                    if self
                        .op
                        .matches(&AttributeValue::String(kind.to_string()), clause_value)
                    {
                        return Ok(self.maybe_negate(true));
                    }
                }
            }
            return Ok(self.maybe_negate(false));
        }

        if let Some(actual_context) = context.as_kind(&self.context_kind) {
            return match actual_context.get_value(&self.attribute) {
                None | Some(AttributeValue::Null) => Ok(false),
                Some(AttributeValue::Array(context_values)) => {
                    for clause_value in &self.values {
                        for context_value in context_values.iter() {
                            if self.op.matches(context_value, clause_value) {
                                return Ok(self.maybe_negate(true));
                            }
                        }
                    }

                    Ok(self.maybe_negate(false))
                }
                Some(context_value) => {
                    if self
                        .values
                        .iter()
                        .any(|clause_value| self.op.matches(&context_value, clause_value))
                    {
                        return Ok(self.maybe_negate(true));
                    }
                    Ok(self.maybe_negate(false))
                }
            };
        }

        Ok(false)
    }

    #[cfg(test)]
    // Use when matching a clause that has an associated context kind.
    pub(crate) fn new_match(reference: Reference, value: AttributeValue, kind: Kind) -> Self {
        Self {
            attribute: reference,
            negate: false,
            op: Op::Matches,
            values: vec![value],
            context_kind: kind,
        }
    }

    #[cfg(test)]
    // Use when matching a clause that isn't context-aware.
    pub(crate) fn new_context_oblivious_match(reference: Reference, value: AttributeValue) -> Self {
        Self {
            attribute: reference,
            negate: false,
            op: Op::Matches,
            values: vec![value],
            context_kind: Kind::default(),
        }
    }
}

impl FlagRule {
    /// Determines if a context matches the provided flag rule.
    ///
    /// A context will match if all flag clauses match; otherwise, this method returns false.
    pub(crate) fn matches(
        &self,
        context: &Context,
        store: &dyn Store,
        evaluation_stack: &mut EvaluationStack,
    ) -> Result<bool, String> {
        // rules match if _all_ of their clauses do
        for clause in &self.clauses {
            let result = clause.matches(context, store, evaluation_stack)?;
            if !result {
                return Ok(false);
            }
        }

        Ok(true)
    }

    #[cfg(test)]
    pub(crate) fn new_segment_match(segment_keys: Vec<&str>, kind: Kind) -> Self {
        Self {
            id: "rule".to_string(),
            clauses: vec![Clause {
                attribute: Reference::new("key"),
                negate: false,
                op: Op::SegmentMatch,
                values: segment_keys
                    .iter()
                    .map(|key| AttributeValue::String(key.to_string()))
                    .collect(),
                context_kind: kind,
            }],
            variation_or_rollout: VariationOrRollout::Variation { variation: 1 },
            track_events: false,
        }
    }
}

impl Op {
    fn matches(&self, lhs: &AttributeValue, rhs: &AttributeValue) -> bool {
        match self {
            Op::In => lhs == rhs,

            // string ops
            Op::StartsWith => string_op(lhs, rhs, |l, r| l.starts_with(r)),
            Op::EndsWith => string_op(lhs, rhs, |l, r| l.ends_with(r)),
            Op::Contains => string_op(lhs, rhs, |l, r| l.contains(r)),
            Op::Matches => string_op(lhs, rhs, |l, r| match Regex::new(r) {
                Ok(re) => re.is_match(l),
                Err(e) => {
                    warn!("Invalid regex for 'matches' operator ({}): {}", e, l);
                    false
                }
            }),

            // numeric ops
            Op::LessThan => numeric_op(lhs, rhs, |l, r| l < r),
            Op::LessThanOrEqual => numeric_op(lhs, rhs, |l, r| l <= r),
            Op::GreaterThan => numeric_op(lhs, rhs, |l, r| l > r),
            Op::GreaterThanOrEqual => numeric_op(lhs, rhs, |l, r| l >= r),

            Op::Before => time_op(lhs, rhs, |l, r| l < r),
            Op::After => time_op(lhs, rhs, |l, r| l > r),

            Op::SegmentMatch => {
                error!("segmentMatch operator should be special-cased, shouldn't get here");
                false
            }

            Op::SemVerEqual => semver_op(lhs, rhs, |l, r| l == r),
            Op::SemVerLessThan => semver_op(lhs, rhs, |l, r| l < r),
            Op::SemVerGreaterThan => semver_op(lhs, rhs, |l, r| l > r),
            Op::Unknown => false,
        }
    }
}

fn string_op<F: Fn(&str, &str) -> bool>(lhs: &AttributeValue, rhs: &AttributeValue, f: F) -> bool {
    match (lhs.as_str(), rhs.as_str()) {
        (Some(l), Some(r)) => f(l, r),
        _ => false,
    }
}

fn numeric_op<F: Fn(f64, f64) -> bool>(lhs: &AttributeValue, rhs: &AttributeValue, f: F) -> bool {
    match (lhs.to_f64(), rhs.to_f64()) {
        (Some(l), Some(r)) => f(l, r),
        _ => false,
    }
}

fn time_op<F: Fn(chrono::DateTime<Utc>, chrono::DateTime<Utc>) -> bool>(
    lhs: &AttributeValue,
    rhs: &AttributeValue,
    f: F,
) -> bool {
    match (lhs.to_datetime(), rhs.to_datetime()) {
        (Some(l), Some(r)) => f(l, r),
        _ => false,
    }
}

fn semver_op<F: Fn(semver::Version, semver::Version) -> bool>(
    lhs: &AttributeValue,
    rhs: &AttributeValue,
    f: F,
) -> bool {
    match (lhs.as_semver(), rhs.as_semver()) {
        (Some(l), Some(r)) => f(l, r),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{flag::Flag, ContextBuilder, Segment};
    use assert_json_diff::assert_json_eq;
    use maplit::hashmap;
    use proptest::prelude::*;
    use serde_json::json;
    use std::collections::HashMap;
    use std::time::SystemTime;
    struct TestStore;
    use crate::proptest_generators::*;

    impl Store for TestStore {
        fn flag(&self, _flag_key: &str) -> Option<Flag> {
            None
        }
        fn segment(&self, _segment_key: &str) -> Option<Segment> {
            None
        }
    }

    fn astring(s: &str) -> AttributeValue {
        AttributeValue::String(s.into())
    }
    fn anum(f: f64) -> AttributeValue {
        AttributeValue::Number(f)
    }

    #[test]
    fn test_op_in() {
        // strings
        assert!(Op::In.matches(&astring("foo"), &astring("foo")));

        assert!(!Op::In.matches(&astring("foo"), &astring("bar")));
        assert!(
            !Op::In.matches(&astring("Foo"), &astring("foo")),
            "case sensitive"
        );

        // numbers
        assert!(Op::In.matches(&anum(42.0), &anum(42.0)));
        assert!(!Op::In.matches(&anum(42.0), &anum(3.0)));
        assert!(Op::In.matches(&anum(0.0), &anum(-0.0)));

        // arrays
        assert!(Op::In.matches(&vec![0.0].into(), &vec![0.0].into()));
        assert!(!Op::In.matches(&vec![0.0, 1.0].into(), &vec![0.0].into()));
        assert!(!Op::In.matches(&vec![0.0].into(), &vec![0.0, 1.0].into()));
        assert!(!Op::In.matches(&anum(0.0), &vec![0.0].into()));
        assert!(!Op::In.matches(&vec![0.0].into(), &anum(0.0)));

        // objects
        assert!(Op::In.matches(&hashmap! {"x" => 0.0}.into(), &hashmap! {"x" => 0.0}.into()));
        assert!(!Op::In.matches(
            &hashmap! {"x" => 0.0, "y" => 1.0}.into(),
            &hashmap! {"x" => 0.0}.into()
        ));
        assert!(!Op::In.matches(
            &hashmap! {"x" => 0.0}.into(),
            &hashmap! {"x" => 0.0, "y" => 1.0}.into()
        ));
        assert!(!Op::In.matches(&anum(0.0), &hashmap! {"x" => 0.0}.into()));
        assert!(!Op::In.matches(&hashmap! {"x" => 0.0}.into(), &anum(0.0)));
    }

    #[test]
    fn test_op_starts_with() {
        // degenerate cases
        assert!(Op::StartsWith.matches(&astring(""), &astring("")));
        assert!(Op::StartsWith.matches(&astring("a"), &astring("")));
        assert!(Op::StartsWith.matches(&astring("a"), &astring("a")));

        // test asymmetry
        assert!(Op::StartsWith.matches(&astring("food"), &astring("foo")));
        assert!(!Op::StartsWith.matches(&astring("foo"), &astring("food")));

        assert!(
            !Op::StartsWith.matches(&astring("Food"), &astring("foo")),
            "case sensitive"
        );
    }

    #[test]
    fn test_op_ends_with() {
        // degenerate cases
        assert!(Op::EndsWith.matches(&astring(""), &astring("")));
        assert!(Op::EndsWith.matches(&astring("a"), &astring("")));
        assert!(Op::EndsWith.matches(&astring("a"), &astring("a")));

        // test asymmetry
        assert!(Op::EndsWith.matches(&astring("food"), &astring("ood")));
        assert!(!Op::EndsWith.matches(&astring("ood"), &astring("food")));

        assert!(
            !Op::EndsWith.matches(&astring("FOOD"), &astring("ood")),
            "case sensitive"
        );
    }

    #[test]
    fn test_op_contains() {
        // degenerate cases
        assert!(Op::Contains.matches(&astring(""), &astring("")));
        assert!(Op::Contains.matches(&astring("a"), &astring("")));
        assert!(Op::Contains.matches(&astring("a"), &astring("a")));

        // test asymmetry
        assert!(Op::Contains.matches(&astring("food"), &astring("oo")));
        assert!(!Op::Contains.matches(&astring("oo"), &astring("food")));

        assert!(
            !Op::Contains.matches(&astring("FOOD"), &astring("oo")),
            "case sensitive"
        );
    }

    #[test]
    fn test_op_matches() {
        fn should_match(text: &str, pattern: &str) {
            assert!(
                Op::Matches.matches(&astring(text), &astring(pattern)),
                "`{}` should match `{}`",
                text,
                pattern
            );
        }

        fn should_not_match(text: &str, pattern: &str) {
            assert!(
                !Op::Matches.matches(&astring(text), &astring(pattern)),
                "`{}` should not match `{}`",
                text,
                pattern
            );
        }

        should_match("", "");
        should_match("a", "");
        should_match("a", "a");
        should_match("a", ".");
        should_match("hello world", "hello.*rld");
        should_match("hello world", "hello.*orl");
        should_match("hello world", "l+");
        should_match("hello world", "(world|planet)");

        should_not_match("", ".");
        should_not_match("", r"\");
        should_not_match("hello world", "aloha");
        should_not_match("hello world", "***bad regex");
    }

    #[test]
    fn test_ops_numeric() {
        // basic numeric comparisons
        assert!(Op::LessThan.matches(&anum(0.0), &anum(1.0)));
        assert!(!Op::LessThan.matches(&anum(0.0), &anum(0.0)));
        assert!(!Op::LessThan.matches(&anum(1.0), &anum(0.0)));

        assert!(Op::GreaterThan.matches(&anum(1.0), &anum(0.0)));
        assert!(!Op::GreaterThan.matches(&anum(0.0), &anum(0.0)));
        assert!(!Op::GreaterThan.matches(&anum(0.0), &anum(1.0)));

        assert!(Op::LessThanOrEqual.matches(&anum(0.0), &anum(1.0)));
        assert!(Op::LessThanOrEqual.matches(&anum(0.0), &anum(0.0)));
        assert!(!Op::LessThanOrEqual.matches(&anum(1.0), &anum(0.0)));

        assert!(Op::GreaterThanOrEqual.matches(&anum(1.0), &anum(0.0)));
        assert!(Op::GreaterThanOrEqual.matches(&anum(0.0), &anum(0.0)));
        assert!(!Op::GreaterThanOrEqual.matches(&anum(0.0), &anum(1.0)));

        // no conversions
        assert!(!Op::LessThan.matches(&astring("0"), &anum(1.0)));
        assert!(!Op::LessThan.matches(&anum(0.0), &astring("1")));
    }

    #[test]
    fn test_ops_time() {
        let today = SystemTime::now();
        let today_millis = today
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis() as f64;
        let yesterday_millis = today_millis - 86_400_000_f64;

        // basic UNIX timestamp comparisons
        assert!(Op::Before.matches(&anum(yesterday_millis), &anum(today_millis)));
        assert!(!Op::Before.matches(&anum(today_millis), &anum(yesterday_millis)));
        assert!(!Op::Before.matches(&anum(today_millis), &anum(today_millis)));

        assert!(Op::After.matches(&anum(today_millis), &anum(yesterday_millis)));
        assert!(!Op::After.matches(&anum(yesterday_millis), &anum(today_millis)));
        assert!(!Op::After.matches(&anum(today_millis), &anum(today_millis)));

        // numeric strings don't get converted to millis
        assert!(!Op::Before.matches(&astring(&yesterday_millis.to_string()), &anum(today_millis)));
        assert!(!Op::After.matches(&anum(today_millis), &astring(&yesterday_millis.to_string())));

        // date-formatted strings get parsed
        assert!(Op::Before.matches(
            &astring("2019-11-19T17:29:00.000000-07:00"),
            &anum(today_millis)
        ));
        assert!(
            Op::Before.matches(&astring("2019-11-19T17:29:00-07:00"), &anum(today_millis)),
            "fractional seconds part is optional"
        );

        assert!(Op::After.matches(
            &anum(today_millis),
            &astring("2019-11-19T17:29:00.000000-07:00")
        ));

        // nonsense strings don't match
        assert!(!Op::Before.matches(&astring("fish"), &anum(today_millis)));
        assert!(!Op::After.matches(&anum(today_millis), &astring("fish")));
    }

    #[test]
    fn test_semver_ops() {
        assert!(Op::SemVerEqual.matches(&astring("2.0.0"), &astring("2.0.0")));

        assert!(
            Op::SemVerEqual.matches(&astring("2.0"), &astring("2.0.0")),
            "we allow missing components (filled in with zeroes)"
        );
        assert!(
            Op::SemVerEqual.matches(&astring("2"), &astring("2.0.0")),
            "we allow missing components (filled in with zeroes)"
        );

        assert!(!Op::SemVerEqual.matches(&astring("2.0.0"), &astring("3.0.0")));
        assert!(!Op::SemVerEqual.matches(&astring("2.0.0"), &astring("2.1.0")));
        assert!(!Op::SemVerEqual.matches(&astring("2.0.0"), &astring("2.0.1")));

        assert!(Op::SemVerGreaterThan.matches(&astring("3.0.0"), &astring("2.0.0")));
        assert!(Op::SemVerGreaterThan.matches(&astring("2.1.0"), &astring("2.0.0")));
        assert!(Op::SemVerGreaterThan.matches(&astring("2.0.1"), &astring("2.0.0")));
        assert!(Op::SemVerGreaterThan
            .matches(&astring("2.0.0-rc.10.green"), &astring("2.0.0-rc.2.green")));
        assert!(
            Op::SemVerGreaterThan.matches(&astring("2.0.0-rc.2.red"), &astring("2.0.0-rc.2.green")),
            "red > green"
        );
        assert!(
            Op::SemVerGreaterThan
                .matches(&astring("2.0.0-rc.2.green.1"), &astring("2.0.0-rc.2.green")),
            "adding more version components makes it greater"
        );

        assert!(!Op::SemVerGreaterThan.matches(&astring("2.0.0"), &astring("2.0.0")));
        assert!(!Op::SemVerGreaterThan.matches(&astring("1.9.0"), &astring("2.0.0")));
        assert!(
            !Op::SemVerGreaterThan.matches(&astring("2.0.0-rc"), &astring("2.0.0")),
            "prerelease version < released version"
        );
        assert!(
            !Op::SemVerGreaterThan.matches(&astring("2.0.0+build"), &astring("2.0.0")),
            "build metadata is ignored, these versions are equal"
        );

        assert!(!Op::SemVerEqual.matches(&astring("2.0.0"), &astring("200")));

        // we don't convert
        assert!(!Op::SemVerEqual.matches(&astring("2.0.0"), &anum(2.0)));
    }

    #[test]
    fn test_clause_matches() {
        let one_val_clause = Clause {
            attribute: Reference::new("a"),
            negate: false,
            op: Op::In,
            values: vec!["foo".into()],
            context_kind: Kind::default(),
        };
        let many_val_clause = Clause {
            attribute: Reference::new("a"),
            negate: false,
            op: Op::In,
            values: vec!["foo".into(), "bar".into()],
            context_kind: Kind::default(),
        };
        let negated_clause = Clause {
            attribute: Reference::new("a"),
            negate: true,
            op: Op::In,
            values: vec!["foo".into()],
            context_kind: Kind::default(),
        };
        let negated_many_val_clause = Clause {
            attribute: Reference::new("a"),
            negate: true,
            op: Op::In,
            values: vec!["foo".into(), "bar".into()],
            context_kind: Kind::default(),
        };
        let key_clause = Clause {
            attribute: Reference::new("key"),
            negate: false,
            op: Op::In,
            values: vec!["matching".into()],
            context_kind: Kind::default(),
        };

        let mut context_builder = ContextBuilder::new("without");
        let context_without_attribute = context_builder.build().expect("Failed to build context");

        context_builder
            .key("matching")
            .set_value("a", AttributeValue::String("foo".to_string()));
        let matching_context = context_builder.build().expect("Failed to build context");

        context_builder
            .key("non-matching")
            .set_value("a", AttributeValue::String("lol".to_string()));
        let non_matching_context = context_builder.build().expect("Failed to build context");

        let mut evaluation_stack = EvaluationStack::default();

        assert!(one_val_clause
            .matches(&matching_context, &TestStore {}, &mut evaluation_stack)
            .unwrap());
        assert!(!one_val_clause
            .matches(&non_matching_context, &TestStore {}, &mut evaluation_stack)
            .unwrap());
        assert!(!one_val_clause
            .matches(
                &context_without_attribute,
                &TestStore {},
                &mut evaluation_stack
            )
            .unwrap());

        assert!(!negated_clause
            .matches(&matching_context, &TestStore {}, &mut evaluation_stack)
            .unwrap());
        assert!(negated_clause
            .matches(&non_matching_context, &TestStore {}, &mut evaluation_stack)
            .unwrap());

        assert!(
            !negated_clause
                .matches(
                    &context_without_attribute,
                    &TestStore {},
                    &mut evaluation_stack
                )
                .unwrap(),
            "targeting missing attribute does not match even when negated"
        );

        assert!(
            many_val_clause
                .matches(&matching_context, &TestStore {}, &mut evaluation_stack)
                .unwrap(),
            "requires only one of the values"
        );
        assert!(!many_val_clause
            .matches(&non_matching_context, &TestStore {}, &mut evaluation_stack)
            .unwrap());
        assert!(!many_val_clause
            .matches(
                &context_without_attribute,
                &TestStore {},
                &mut evaluation_stack
            )
            .unwrap());

        assert!(
            !negated_many_val_clause
                .matches(&matching_context, &TestStore {}, &mut evaluation_stack)
                .unwrap(),
            "requires all values are missing"
        );
        assert!(negated_many_val_clause
            .matches(&non_matching_context, &TestStore {}, &mut evaluation_stack)
            .unwrap());

        assert!(
            !negated_many_val_clause
                .matches(
                    &context_without_attribute,
                    &TestStore {},
                    &mut evaluation_stack
                )
                .unwrap(),
            "targeting missing attribute does not match even when negated"
        );

        assert!(
            key_clause
                .matches(&matching_context, &TestStore {}, &mut evaluation_stack)
                .unwrap(),
            "should match key"
        );
        assert!(
            !key_clause
                .matches(&non_matching_context, &TestStore {}, &mut evaluation_stack)
                .unwrap(),
            "should not match non-matching key"
        );

        context_builder.key("with-many").set_value(
            "a",
            AttributeValue::Array(vec![
                AttributeValue::String("foo".to_string()),
                AttributeValue::String("bar".to_string()),
                AttributeValue::String("lol".to_string()),
            ]),
        );
        let context_with_many = context_builder.build().expect("Failed to build context");

        assert!(one_val_clause
            .matches(&context_with_many, &TestStore {}, &mut evaluation_stack)
            .unwrap());
        assert!(many_val_clause
            .matches(&context_with_many, &TestStore {}, &mut evaluation_stack)
            .unwrap());

        assert!(!negated_clause
            .matches(&context_with_many, &TestStore {}, &mut evaluation_stack)
            .unwrap());
        assert!(!negated_many_val_clause
            .matches(&context_with_many, &TestStore {}, &mut evaluation_stack)
            .unwrap());
    }

    struct AttributeTestCase {
        matching_context: Context,
        non_matching_context: Context,
        context_without_attribute: Option<Context>,
    }

    #[test]
    fn test_clause_matches_attributes() {
        let tests: HashMap<&str, AttributeTestCase> = hashmap! {
            "key" => AttributeTestCase {
                matching_context: ContextBuilder::new("match").build().unwrap(),
                non_matching_context: ContextBuilder::new("nope").build().unwrap(),
                context_without_attribute: None,
            },
            "name" => AttributeTestCase {
                matching_context: ContextBuilder::new("matching").name("match").build().unwrap(),
                non_matching_context: ContextBuilder::new("non-matching").name("nope").build().unwrap(),
                context_without_attribute: Some(ContextBuilder::new("without-attribute").build().unwrap()),
            },
        };

        let mut evaluation_stack = EvaluationStack::default();

        for (attr, test_case) in tests {
            let clause = Clause {
                attribute: Reference::new(attr),
                negate: false,
                op: Op::In,
                values: vec!["match".into()],
                context_kind: Kind::default(),
            };

            assert!(
                clause
                    .matches(
                        &test_case.matching_context,
                        &TestStore {},
                        &mut evaluation_stack
                    )
                    .unwrap(),
                "should match {}",
                attr
            );
            assert!(
                !clause
                    .matches(
                        &test_case.non_matching_context,
                        &TestStore {},
                        &mut evaluation_stack
                    )
                    .unwrap(),
                "should not match non-matching {}",
                attr
            );
            if let Some(context_without_attribute) = test_case.context_without_attribute {
                assert!(
                    !clause
                        .matches(
                            &context_without_attribute,
                            &TestStore {},
                            &mut evaluation_stack
                        )
                        .unwrap(),
                    "should not match user with null {}",
                    attr
                );
            }
        }
    }

    #[test]
    fn test_clause_matches_anonymous_attribute() {
        let clause = Clause {
            attribute: Reference::new("anonymous"),
            negate: false,
            op: Op::In,
            values: vec![true.into()],
            context_kind: Kind::default(),
        };

        let anon_context = ContextBuilder::new("anon").anonymous(true).build().unwrap();
        let non_anon_context = ContextBuilder::new("nonanon")
            .anonymous(false)
            .build()
            .unwrap();
        let implicitly_non_anon_context = ContextBuilder::new("implicit").build().unwrap();

        let mut evaluation_stack = EvaluationStack::default();
        assert!(clause
            .matches(&anon_context, &TestStore {}, &mut evaluation_stack)
            .unwrap());
        assert!(!clause
            .matches(&non_anon_context, &TestStore {}, &mut evaluation_stack)
            .unwrap());
        assert!(!clause
            .matches(
                &implicitly_non_anon_context,
                &TestStore {},
                &mut evaluation_stack
            )
            .unwrap());
    }

    #[test]
    fn test_clause_matches_custom_attributes() {
        // check we can have an attribute called "custom"
        for attr in &["custom", "custom1"] {
            let clause = Clause {
                attribute: Reference::new(attr),
                negate: false,
                op: Op::In,
                values: vec!["match".into()],
                context_kind: Kind::default(),
            };

            let matching_context = ContextBuilder::new("matching")
                .set_value(attr, AttributeValue::String("match".into()))
                .build()
                .unwrap();
            let non_matching_context = ContextBuilder::new("non-matching")
                .set_value(attr, AttributeValue::String("nope".into()))
                .build()
                .unwrap();
            let context_without_attribute = ContextBuilder::new("without_attribute")
                .set_value(attr, AttributeValue::Null)
                .build()
                .unwrap();

            let mut evaluation_stack = EvaluationStack::default();
            assert!(
                clause
                    .matches(&matching_context, &TestStore {}, &mut evaluation_stack)
                    .unwrap(),
                "should match {}",
                attr
            );
            assert!(
                !clause
                    .matches(&non_matching_context, &TestStore {}, &mut evaluation_stack)
                    .unwrap(),
                "should not match non-matching {}",
                attr
            );
            assert!(
                !clause
                    .matches(
                        &context_without_attribute,
                        &TestStore {},
                        &mut evaluation_stack
                    )
                    .unwrap(),
                "should not match user with null {}",
                attr
            );
        }
    }

    #[test]
    fn test_null_attribute() {
        let context_null_attr = ContextBuilder::new("key")
            .set_value("attr", AttributeValue::Null)
            .build()
            .unwrap();

        let context_missing_attr = ContextBuilder::new("key").build().unwrap();

        let clause_values = vec![
            AttributeValue::Bool(true),
            AttributeValue::Bool(false),
            AttributeValue::Number(1.5),
            AttributeValue::Number(1.0),
            AttributeValue::Null,
            AttributeValue::String("abc".to_string()),
            AttributeValue::Array(vec![
                AttributeValue::String("def".to_string()),
                AttributeValue::Null,
            ]),
        ];

        for op in &[
            Op::In,
            Op::StartsWith,
            Op::EndsWith,
            Op::Contains,
            Op::Matches,
            Op::LessThan,
            Op::LessThanOrEqual,
            Op::GreaterThan,
            Op::GreaterThanOrEqual,
            Op::Before,
            Op::After,
            Op::SemVerEqual,
            Op::SemVerGreaterThan,
            Op::SemVerLessThan,
        ] {
            for neg in &[true, false] {
                let clause = Clause {
                    attribute: Reference::new("attr"),
                    negate: *neg,
                    op: *op,
                    values: clause_values.clone(),
                    context_kind: Kind::default(),
                };
                let mut evaluation_stack = EvaluationStack::default();
                assert!(
                    !clause
                        .matches(&context_null_attr, &TestStore {}, &mut evaluation_stack)
                        .unwrap(),
                    "Null attribute matches operator {:?} when {}negated",
                    clause.op,
                    if *neg { "" } else { "not " },
                );
                assert!(
                    !clause
                        .matches(&context_missing_attr, &TestStore {}, &mut evaluation_stack)
                        .unwrap(),
                    "Missing attribute matches operator {:?} when {}negated",
                    clause.op,
                    if *neg { "" } else { "not " },
                );
            }
        }
    }

    // The following test cases are ported from the Go implementation:
    // https://github.com/launchdarkly/go-server-sdk-evaluation/blob/v1/ldmodel/match_clause_operator_test.go#L28-L155
    fn clause_test_case<S, T>(op: Op, context_value: S, clause_value: T, expected: bool)
    where
        AttributeValue: From<S>,
        AttributeValue: From<T>,
        S: Clone,
        T: Clone,
    {
        let clause = Clause {
            attribute: Reference::new("attr"),
            negate: false,
            op,
            values: match clause_value.into() {
                AttributeValue::Array(vec) => vec,
                other => vec![other],
            },
            context_kind: Kind::default(),
        };

        let context = ContextBuilder::new("key")
            .set_value("attr", context_value.into())
            .build()
            .unwrap();

        let mut evaluation_stack = EvaluationStack::default();
        assert_eq!(
            clause
                .matches(&context, &TestStore {}, &mut evaluation_stack)
                .unwrap(),
            expected,
            "{:?} {:?} {:?} should be {}",
            context.get_value(&Reference::new("attr")).unwrap(),
            clause.op,
            clause.values,
            &expected
        );
    }

    #[test]
    fn match_is_false_on_invalid_reference() {
        let clause = Clause {
            attribute: Reference::new("/"),
            negate: false,
            op: Op::In,
            values: vec![],
            context_kind: Kind::default(),
        };

        let context = ContextBuilder::new("key")
            .set_value("attr", true.into())
            .build()
            .unwrap();
        let mut evaluation_stack = EvaluationStack::default();
        assert!(clause
            .matches(&context, &TestStore {}, &mut evaluation_stack)
            .is_err());
    }

    #[test]
    fn match_is_false_no_context_matches() {
        let clause = Clause {
            attribute: Reference::new("attr"),
            negate: false,
            op: Op::In,
            values: vec![true.into()],
            context_kind: Kind::default(),
        };

        let context = ContextBuilder::new("key")
            .kind("org")
            .set_value("attr", true.into())
            .build()
            .unwrap();
        let mut evaluation_stack = EvaluationStack::default();
        assert!(!clause
            .matches(&context, &TestStore {}, &mut evaluation_stack)
            .unwrap());
    }

    #[test]
    fn test_numeric_clauses() {
        clause_test_case(Op::In, 99, 99, true);
        clause_test_case(Op::In, 99.0, 99, true);
        clause_test_case(Op::In, 99, 99.0, true);
        clause_test_case(Op::In, 99, vec![99, 98, 97, 96], true);
        clause_test_case(Op::In, 99.0001, 99.0001, true);
        clause_test_case(Op::In, 99.0001, vec![99.0001, 98.0, 97.0, 96.0], true);
        clause_test_case(Op::LessThan, 1, 1.99999, true);
        clause_test_case(Op::LessThan, 1.99999, 1, false);
        clause_test_case(Op::LessThan, 1, 2, true);
        clause_test_case(Op::LessThanOrEqual, 1, 1.0, true);
        clause_test_case(Op::GreaterThan, 2, 1.99999, true);
        clause_test_case(Op::GreaterThan, 1.99999, 2, false);
        clause_test_case(Op::GreaterThan, 2, 1, true);
        clause_test_case(Op::GreaterThanOrEqual, 1, 1.0, true);
    }

    #[test]
    fn test_string_clauses() {
        clause_test_case(Op::In, "x", "x", true);
        clause_test_case(Op::In, "x", vec!["x", "a", "b", "c"], true);
        clause_test_case(Op::In, "x", "xyz", false);
        clause_test_case(Op::StartsWith, "xyz", "x", true);
        clause_test_case(Op::StartsWith, "x", "xyz", false);
        clause_test_case(Op::EndsWith, "xyz", "z", true);
        clause_test_case(Op::EndsWith, "z", "xyz", false);
        clause_test_case(Op::Contains, "xyz", "y", true);
        clause_test_case(Op::Contains, "y", "xyz", false);
    }

    #[test]
    fn test_mixed_string_and_numbers() {
        clause_test_case(Op::In, "99", 99, false);
        clause_test_case(Op::In, 99, "99", false);
        clause_test_case(Op::Contains, "99", 99, false);
        clause_test_case(Op::StartsWith, "99", 99, false);
        clause_test_case(Op::EndsWith, "99", 99, false);
        clause_test_case(Op::LessThanOrEqual, "99", 99, false);
        clause_test_case(Op::LessThanOrEqual, 99, "99", false);
        clause_test_case(Op::GreaterThanOrEqual, "99", 99, false);
        clause_test_case(Op::GreaterThanOrEqual, 99, "99", false);
    }

    #[test]
    fn test_boolean_equality() {
        clause_test_case(Op::In, true, true, true);
        clause_test_case(Op::In, false, false, true);
        clause_test_case(Op::In, true, false, false);
        clause_test_case(Op::In, false, true, false);
        clause_test_case(Op::In, true, vec![false, true], true);
    }

    #[test]
    fn test_array_equality() {
        // note that the user value must be an array *of arrays*, because a single-level
        // array is interpreted as "any of these values"
        clause_test_case(Op::In, vec![vec!["x"]], vec![vec!["x"]], true);
        clause_test_case(Op::In, vec![vec!["x"]], vec!["x"], false);
        clause_test_case(
            Op::In,
            vec![vec!["x"]],
            vec![vec!["x"], vec!["a"], vec!["b"]],
            true,
        );
    }

    #[test]
    fn test_object_equality() {
        clause_test_case(Op::In, hashmap! {"x" => "1"}, hashmap! {"x" => "1"}, true);
        clause_test_case(
            Op::In,
            hashmap! {"x" => "1"},
            vec![
                hashmap! {"x" => "1"},
                hashmap! {"a" => "2"},
                hashmap! {"b" => "3"},
            ],
            true,
        );
    }

    #[test]
    fn test_regex_match() {
        clause_test_case(Op::Matches, "hello world", "hello.*rld", true);
        clause_test_case(Op::Matches, "hello world", "hello.*orl", true);
        clause_test_case(Op::Matches, "hello world", "l+", true);
        clause_test_case(Op::Matches, "hello world", "(world|planet)", true);
        clause_test_case(Op::Matches, "hello world", "aloha", false);
        clause_test_case(Op::Matches, "hello world", "***bad regex", false);
    }

    #[test]
    fn test_date_clauses() {
        const DATE_STR1: &str = "2017-12-06T00:00:00.000-07:00";
        const DATE_STR2: &str = "2017-12-06T00:01:01.000-07:00";
        const DATE_MS1: i64 = 10000000;
        const DATE_MS2: i64 = 10000001;
        const INVALID_DATE: &str = "hey what's this?";

        clause_test_case(Op::Before, DATE_STR1, DATE_STR2, true);
        clause_test_case(Op::Before, DATE_MS1, DATE_MS2, true);
        clause_test_case(Op::Before, DATE_STR2, DATE_STR1, false);
        clause_test_case(Op::Before, DATE_MS2, DATE_MS1, false);
        clause_test_case(Op::Before, DATE_STR1, DATE_STR1, false);
        clause_test_case(Op::Before, DATE_MS1, DATE_MS1, false);
        clause_test_case(Op::Before, AttributeValue::Null, DATE_STR1, false);
        clause_test_case(Op::Before, DATE_STR1, INVALID_DATE, false);
        clause_test_case(Op::After, DATE_STR2, DATE_STR1, true);
        clause_test_case(Op::After, DATE_MS2, DATE_MS1, true);
        clause_test_case(Op::After, DATE_STR1, DATE_STR2, false);
        clause_test_case(Op::After, DATE_MS1, DATE_MS2, false);
        clause_test_case(Op::After, DATE_STR1, DATE_STR1, false);
        clause_test_case(Op::After, DATE_MS1, DATE_MS1, false);
        clause_test_case(Op::After, AttributeValue::Null, DATE_STR1, false);
        clause_test_case(Op::After, DATE_STR1, INVALID_DATE, false);
    }

    #[test]
    fn test_semver_clauses() {
        clause_test_case(Op::SemVerEqual, "2.0.0", "2.0.0", true);
        clause_test_case(Op::SemVerEqual, "2.0", "2.0.0", true);
        clause_test_case(Op::SemVerEqual, "2-rc1", "2.0.0-rc1", true);
        clause_test_case(Op::SemVerEqual, "2+build2", "2.0.0+build2", true);
        clause_test_case(Op::SemVerEqual, "2.0.0", "2.0.1", false);
        clause_test_case(Op::SemVerLessThan, "2.0.0", "2.0.1", true);
        clause_test_case(Op::SemVerLessThan, "2.0", "2.0.1", true);
        clause_test_case(Op::SemVerLessThan, "2.0.1", "2.0.0", false);
        clause_test_case(Op::SemVerLessThan, "2.0.1", "2.0", false);
        clause_test_case(Op::SemVerLessThan, "2.0.1", "xbad%ver", false);
        clause_test_case(Op::SemVerLessThan, "2.0.0-rc", "2.0.0-rc.beta", true);
        clause_test_case(Op::SemVerGreaterThan, "2.0.1", "2.0", true);
        clause_test_case(Op::SemVerGreaterThan, "10.0.1", "2.0", true);
        clause_test_case(Op::SemVerGreaterThan, "2.0.0", "2.0.1", false);
        clause_test_case(Op::SemVerGreaterThan, "2.0", "2.0.1", false);
        clause_test_case(Op::SemVerGreaterThan, "2.0.1", "xbad%ver", false);
        clause_test_case(Op::SemVerGreaterThan, "2.0.0-rc.1", "2.0.0-rc.0", true);
    }

    #[test]
    fn clause_deserialize_with_attribute_missing_causes_error() {
        let attribute_missing = json!({
            "op" : "in",
            "values" : [],
        });
        assert!(serde_json::from_value::<IntermediateClause>(attribute_missing).is_err());
    }

    #[test]
    fn clause_deserialize_with_op_missing_causes_error() {
        let op_missing = json!({
            "values" : [],
            "attribute" : "",
        });
        assert!(serde_json::from_value::<IntermediateClause>(op_missing).is_err());
    }

    #[test]
    fn clause_deserialize_with_values_missing_causes_error() {
        let values_missing = json!({
            "op" : "in",
            "values" : [],
        });
        assert!(serde_json::from_value::<IntermediateClause>(values_missing).is_err());
    }

    #[test]
    fn clause_deserialize_with_required_fields_parses_successfully() {
        let all_required_fields_present = json!({
            "attribute" : "",
            "op" : "in",
            "values" : [],
        });

        assert_eq!(
            serde_json::from_value::<IntermediateClause>(all_required_fields_present).unwrap(),
            IntermediateClause::ContextOblivious(ClauseWithoutKind {
                attribute: AttributeName::default(),
                negate: false,
                op: Op::In,
                values: vec![]
            })
        );
    }

    proptest! {
        #[test]
        fn arbitrary_clause_serialization_rountrip(clause in any_clause()) {
            let json = serde_json::to_value(&clause).expect("a clause should serialize");
            let parsed: Clause = serde_json::from_value(json.clone()).expect("a clause should parse");
            assert_json_eq!(json, parsed);
        }
    }

    #[test]
    fn clause_with_negate_omitted_defaults_to_false() {
        let negate_omitted = json!({
            "attribute" : "",
            "op" : "in",
            "values" : [],
        });

        assert!(
            !serde_json::from_value::<Clause>(negate_omitted)
                .unwrap()
                .negate
        )
    }

    #[test]
    fn clause_with_empty_attribute_defaults_to_invalid_attribute() {
        let empty_attribute = json!({
            "attribute" : "",
            "op" : "in",
            "values" : [],
        });

        let attr = serde_json::from_value::<Clause>(empty_attribute)
            .unwrap()
            .attribute;
        assert_eq!(Reference::default(), attr);
    }

    proptest! {
        #[test]
        fn clause_with_context_kind_implies_attribute_references(arbitrary_attribute in any::<String>()) {
            let with_context_kind = json!({
                "attribute" : arbitrary_attribute,
                "op" : "in",
                "values" : [],
                "contextKind" : "user",
            });

            prop_assert_eq!(
                Reference::new(arbitrary_attribute),
                serde_json::from_value::<Clause>(with_context_kind)
                    .unwrap()
                    .attribute
            )
        }
    }

    proptest! {
        #[test]
        fn clause_without_context_kind_implies_literal_attribute_name(arbitrary_attribute in any_valid_ref_string()) {
            let without_context_kind = json!({
                "attribute" : arbitrary_attribute,
                "op" : "in",
                "values" : [],
            });

            prop_assert_eq!(
                Reference::from(AttributeName::new(arbitrary_attribute)),
                serde_json::from_value::<Clause>(without_context_kind)
                .unwrap()
                .attribute
            );
        }
    }
}
