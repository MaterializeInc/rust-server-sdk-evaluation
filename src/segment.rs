use serde::{Deserialize, Serialize};

use crate::contexts::attribute_reference::AttributeName;
use crate::contexts::context::{BucketPrefix, Kind};
use crate::rule::Clause;
use crate::variation::VariationWeight;
use crate::{Context, EvaluationStack, Reference, Store, Versioned};
use serde_with::skip_serializing_none;

/// Segment describes a group of contexts based on keys and/or matching rules.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    /// The unique key of the segment.
    pub key: String,
    /// A list of context keys that are always matched by this segment.
    pub included: Vec<String>,
    /// A list of context keys that are never matched by this segment, unless the key is also in
    /// included.
    pub excluded: Vec<String>,

    #[serde(default)]
    included_contexts: Vec<SegmentTarget>,
    #[serde(default)]
    excluded_contexts: Vec<SegmentTarget>,

    rules: Vec<SegmentRule>,
    salt: String,

    /// Unbounded is true if this is a segment whose included list is stored separately and is not limited in size.
    /// Currently, the server-side Rust SDK cannot access the context list for this kind of segment; it only works when
    /// flags are being evaluated within the LaunchDarkly service.
    ///
    /// The name is historical: "unbounded segments" was an earlier name for the product feature that is currently
    /// known as "big segments". If unbounded is true, this is a big segment.
    #[serde(default)]
    pub unbounded: bool,
    #[serde(default)]
    generation: Option<i64>,

    /// An integer that is incremented by LaunchDarkly every time the configuration of the segment
    /// is changed.
    pub version: u64,
}

impl Versioned for Segment {
    fn version(&self) -> u64 {
        self.version
    }
}

// SegmentRule describes a rule that determines if a context is part of a segment.
// SegmentRule is deserialized via a helper, IntermediateSegmentRule, because of semantic ambiguity
// of the bucketBy Reference field.
//
// SegmentRule implements Serialize directly without a helper because References can serialize
// themselves without any ambiguity.
#[skip_serializing_none]
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", from = "IntermediateSegmentRule")]
struct SegmentRule {
    // Unique identifier provided by the LaunchDarkly backend for this rule.
    id: Option<String>,
    // The clauses that comprise this rule.
    clauses: Vec<Clause>,
    // A percentage rollout allowing only a subset of contexts to be included in this segment.
    weight: Option<VariationWeight>,
    // Which attribute should be used to distinguish between contexts in a rollout.
    // Can be omitted; evaluation should treat absence as 'key'.
    bucket_by: Option<Reference>,
    // Only present when this segment rule is a rollout, i.e., only present when weight is present.
    rollout_context_kind: Option<Kind>,
}

// SegmentRule is deserialized via IntermediateSegmentRule, taking advantage of
// serde's untagged enum support.
//
// This is necessary because SegmentRules directly contain attribute references, specifically
// the bucketBy field. References require care when deserializing; see the Reference documentation
// for more info.
//
// Serde will attempt deserialization into the first enum variant, and if it fails, the second.
// This implies deserialization will be relatively slower for the second variant.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(untagged)]
enum IntermediateSegmentRule {
    // SegmentRuleWithKind must be listed first in the enum because otherwise SegmentRuleWithoutKind
    // could match the input (by ignoring/discarding the rollout_context_kind field).
    ContextAware(SegmentRuleWithKind),
    ContextOblivious(SegmentRuleWithoutKind),
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct SegmentRuleWithKind {
    id: Option<String>,
    clauses: Vec<Clause>,
    weight: Option<VariationWeight>,
    bucket_by: Option<Reference>,
    rollout_context_kind: Kind,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct SegmentRuleWithoutKind {
    id: Option<String>,
    clauses: Vec<Clause>,
    weight: Option<VariationWeight>,
    bucket_by: Option<AttributeName>,
}

impl From<IntermediateSegmentRule> for SegmentRule {
    fn from(rule: IntermediateSegmentRule) -> SegmentRule {
        match rule {
            IntermediateSegmentRule::ContextAware(fields) => SegmentRule {
                id: fields.id,
                clauses: fields.clauses,
                weight: fields.weight,
                // No transformation is necessary since ContextAware implies this
                // data is using attribute references.
                bucket_by: fields.bucket_by,
                rollout_context_kind: Some(fields.rollout_context_kind),
            },
            IntermediateSegmentRule::ContextOblivious(fields) => SegmentRule {
                id: fields.id,
                clauses: fields.clauses,
                weight: fields.weight,
                // ContextOblivious implies this data is using literal attribute names, so
                // the AttributeName must be converted to a Reference (if present).
                bucket_by: fields.bucket_by.map(Reference::from),
                rollout_context_kind: None,
            },
        }
    }
}

impl Segment {
    /// Determines if the provided context is a part of this segment.
    ///
    /// Inclusion can be determined by specifically listing the context key in the segment, or by
    /// matching any of the rules configured for this segment.
    pub(crate) fn contains(
        &self,
        context: &Context,
        store: &dyn Store,
        evaluation_stack: &mut EvaluationStack,
    ) -> Result<bool, String> {
        if evaluation_stack.segment_chain.contains(&self.key) {
            return Err(format!("segment rule referencing segment {} caused a circular reference; this is probably a temporary condition due to an incomplete update", self.key));
        }

        evaluation_stack.segment_chain.insert(self.key.clone());

        let mut does_contain = false;
        if self.is_contained_in(context, &self.included, &self.included_contexts) {
            does_contain = true;
        } else if self.is_contained_in(context, &self.excluded, &self.excluded_contexts) {
            does_contain = false;
        } else {
            for rule in &self.rules {
                let matches =
                    rule.matches(context, store, &self.key, &self.salt, evaluation_stack)?;
                if matches {
                    does_contain = true;
                    break;
                }
            }
        }

        evaluation_stack.segment_chain.remove(&self.key);

        Ok(does_contain)
    }

    fn is_contained_in(
        &self,
        context: &Context,
        keys: &[String],
        targets: &[SegmentTarget],
    ) -> bool {
        if context.kind().is_user() && targets.is_empty() {
            return keys.iter().any(|k| k == context.key());
        }

        for target in targets {
            if let Some(context) = context.as_kind(&target.context_kind) {
                let key = context.key();
                if target.values.iter().any(|v| v == key) {
                    return true;
                }

                if context.kind().is_user() && keys.iter().any(|k| k == key) {
                    return true;
                }
            }
        }

        false
    }

    /// Retrieve the id representing this big segment.
    ///
    /// This id will either be the segment key if the segment isn't a big segment, or it will be a
    /// combination of the segment key and the segment generation id.
    pub fn unbounded_segment_id(&self) -> String {
        match self.generation {
            None | Some(0) => self.key.clone(),
            Some(generation) => format!("{}.g{}", self.key, generation),
        }
    }
}

impl SegmentRule {
    /// Determines if a context matches the provided segment rule.
    ///
    /// A context will match if all segment clauses match; otherwise, this method returns false.
    pub fn matches(
        &self,
        context: &Context,
        store: &dyn Store,
        key: &str,
        salt: &str,
        evaluation_stack: &mut EvaluationStack,
    ) -> Result<bool, String> {
        // rules match if _all_ of their clauses do
        for clause in &self.clauses {
            let matches = clause.matches(context, store, evaluation_stack)?;
            if !matches {
                return Ok(false);
            }
        }

        match self.weight {
            Some(weight) if weight >= 0.0 => {
                let prefix = BucketPrefix::KeyAndSalt(key, salt);
                let (bucket, _) = context.bucket(
                    &self.bucket_by,
                    prefix,
                    false,
                    self.rollout_context_kind
                        .as_ref()
                        .unwrap_or(&Kind::default()),
                )?;
                Ok(bucket < weight / 100_000.0)
            }
            _ => Ok(true),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SegmentTarget {
    values: Vec<String>,
    context_kind: Kind,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contexts::attribute_reference::Reference;
    use crate::eval::evaluate;
    use crate::{proptest_generators::*, AttributeValue, ContextBuilder, Flag, FlagValue, Store};
    use assert_json_diff::assert_json_eq;
    use proptest::{collection::vec, option::of, prelude::*};
    use serde_json::json;

    prop_compose! {
        // Generate an arbitrary SegmentRule with 0-3 clauses
        fn any_segment_rule()(
            id in of(any::<String>()),
            clauses in vec(any_clause(), 0..3),
            weight in of(any::<f32>()),
            // reference is any_ref(), rather than any_valid_ref(), because we also want
            // coverage of invalid references.
            bucket_by in any_ref(),
            rollout_context_kind in any_kind()
        ) -> SegmentRule {
            SegmentRule {
                id,
                clauses,
                weight,
                bucket_by: Some(bucket_by),
                rollout_context_kind: Some(rollout_context_kind),
            }
        }
    }

    #[test]
    fn handles_contextless_schema() {
        let json = &r#"{
                "key": "segment",
                "included": ["alice"],
                "excluded": ["bob"],
                "rules": [],
                "salt": "salty",
                "version": 1
            }"#
        .to_string();

        let segment: Segment = serde_json::from_str(json).expect("Failed to parse segment");
        assert_eq!(1, segment.included.len());
        assert_eq!(1, segment.excluded.len());

        assert!(segment.included_contexts.is_empty());
        assert!(segment.excluded_contexts.is_empty());
    }

    #[test]
    fn handles_context_schema() {
        let json = &r#"{
                "key": "segment",
                "included": [],
                "excluded": [],
                "includedContexts": [{
                    "values": ["alice", "bob"],
                    "contextKind": "org"
                }],
                "excludedContexts": [{
                    "values": ["cris", "darren"],
                    "contextKind": "org"
                }],
                "rules": [],
                "salt": "salty",
                "version": 1
            }"#
        .to_string();

        let segment: Segment = serde_json::from_str(json).expect("Failed to parse segment");
        assert!(segment.included.is_empty());
        assert!(segment.excluded.is_empty());

        assert_eq!(1, segment.included_contexts.len());
        assert_eq!(1, segment.excluded_contexts.len());
    }

    // Treat a Segment as a Store containing only itself
    type TestStore = Segment;
    impl Store for TestStore {
        fn flag(&self, _flag_key: &str) -> Option<Flag> {
            None
        }
        fn segment(&self, segment_key: &str) -> Option<Segment> {
            if self.key == segment_key {
                Some(self.clone())
            } else {
                None
            }
        }
    }

    fn assert_segment_match(segment: &Segment, context: Context, expected: bool) {
        let store = segment as &TestStore;
        let flag = Flag::new_boolean_flag_with_segment_match(vec![&segment.key], Kind::user());
        let result = evaluate(store, &flag, &context, None);
        assert_eq!(result.value, Some(&FlagValue::Bool(expected)));
    }

    fn new_segment() -> Segment {
        Segment {
            key: "segkey".to_string(),
            included: vec![],
            excluded: vec![],
            included_contexts: vec![],
            excluded_contexts: vec![],
            rules: vec![],
            salt: "salty".to_string(),
            unbounded: false,
            generation: Some(1),
            version: 1,
        }
    }

    fn jane_rule(
        weight: Option<f32>,
        bucket_by: Option<Reference>,
        kind: Option<Kind>,
    ) -> SegmentRule {
        SegmentRule {
            id: None,
            clauses: vec![Clause::new_match(
                Reference::new("name"),
                AttributeValue::String("Jane".to_string()),
                Kind::user(),
            )],
            weight,
            bucket_by,
            rollout_context_kind: kind,
        }
    }

    fn thirty_percent_rule(bucket_by: Option<Reference>, kind: Option<Kind>) -> SegmentRule {
        SegmentRule {
            id: None,
            clauses: vec![Clause::new_match(
                Reference::new("key"),
                AttributeValue::String(".".to_string()),
                Kind::user(),
            )],
            weight: Some(30_000.0),
            bucket_by,
            rollout_context_kind: kind,
        }
    }

    #[test]
    fn segment_rule_parse_only_required_field_is_clauses() {
        let rule: SegmentRule =
            serde_json::from_value(json!({"clauses": []})).expect("should parse");
        assert_eq!(
            rule,
            SegmentRule {
                id: None,
                clauses: vec![],
                weight: None,
                bucket_by: None,
                rollout_context_kind: None,
            }
        );
    }

    #[test]
    fn segment_rule_serialize_omits_optional_fields() {
        let json = json!({"clauses": []});
        let rule: SegmentRule = serde_json::from_value(json.clone()).expect("should parse");
        assert_json_eq!(json, rule);
    }

    proptest! {
        #[test]
        fn segment_rule_parse_references_as_literal_attribute_names_when_context_kind_omitted(
                clause_attr in any_valid_ref_string(),
                bucket_by in any_valid_ref_string()
            ) {
            let omit_context_kind: SegmentRule = serde_json::from_value(json!({
                "id" : "test",
                "clauses":[{
                    "attribute": clause_attr,
                    "negate": false,
                    "op": "matches",
                    "values": ["xyz"],
                }],
                "weight": 10000,
                "bucketBy": bucket_by,
            }))
            .expect("should parse");

             let empty_context_kind: SegmentRule = serde_json::from_value(json!({
                "id" : "test",
                "clauses":[{
                    "attribute": clause_attr,
                    "negate": false,
                    "op": "matches",
                    "values": ["xyz"],
                    "contextKind" : "",
                }],
                "weight": 10000,
                "bucketBy": bucket_by,
            }))
            .expect("should parse");

            let expected = SegmentRule {
                id: Some("test".into()),
                clauses: vec![Clause::new_context_oblivious_match(
                    Reference::from(AttributeName::new(clause_attr)),
                    "xyz".into(),
                )],
                weight: Some(10_000.0),
                bucket_by: Some(Reference::from(AttributeName::new(bucket_by))),
                rollout_context_kind: None,
            };

            prop_assert_eq!(
                omit_context_kind,
                expected.clone()
            );

            prop_assert_eq!(
                empty_context_kind,
                expected
            );
        }
    }

    proptest! {
        #[test]
        fn segment_rule_parse_references_normally_when_context_kind_present(
                clause_attr in any_ref(),
                bucket_by in any_ref()
            ) {
            let rule: SegmentRule = serde_json::from_value(json!({
                "id" : "test",
                "clauses":[{
                    "attribute": clause_attr.to_string(),
                    "negate": false,
                    "op": "matches",
                    "values": ["xyz"],
                    "contextKind" : "user"
                }],
                "weight": 10000,
                "bucketBy": bucket_by.to_string(),
                "rolloutContextKind" : "user"
            }))
            .expect("should parse");

            prop_assert_eq!(
                rule,
                SegmentRule {
                    id: Some("test".into()),
                    clauses: vec![Clause::new_match(
                        clause_attr,
                        "xyz".into(),
                        Kind::user()
                    )],
                    weight: Some(10_000.0),
                    bucket_by: Some(bucket_by),
                    rollout_context_kind: Some(Kind::user()),
                }
            );
        }
    }

    proptest! {
        #[test]
        fn arbitrary_segment_rule_serialization_roundtrip(rule in any_segment_rule()) {
            let json = serde_json::to_value(&rule).expect("an arbitrary segment rule should serialize");
            let parsed: SegmentRule = serde_json::from_value(json.clone()).expect("an arbitrary segment rule should parse");
            assert_json_eq!(json, parsed);
        }
    }

    #[test]
    fn segment_match_clause_falls_through_if_segment_not_found() {
        let mut segment = new_segment();
        segment.included.push("foo".to_string());
        segment.included_contexts.push(SegmentTarget {
            values: vec![],
            context_kind: Kind::user(),
        });
        segment.key = "different-key".to_string();
        let context = ContextBuilder::new("foo").build().unwrap();
        assert_segment_match(&segment, context, true);
    }

    #[test]
    fn can_match_just_one_segment_from_list() {
        let mut segment = new_segment();
        segment.included.push("foo".to_string());
        segment.included_contexts.push(SegmentTarget {
            values: vec![],
            context_kind: Kind::user(),
        });
        let context = ContextBuilder::new("foo").build().unwrap();
        let flag = Flag::new_boolean_flag_with_segment_match(
            vec!["different-segkey", "segkey", "another-segkey"],
            Kind::user(),
        );
        let result = evaluate(&segment, &flag, &context, None);
        assert_eq!(result.value, Some(&FlagValue::Bool(true)));
    }

    #[test]
    fn user_is_explicitly_included_in_segment() {
        let mut segment = new_segment();
        segment.included.push("foo".to_string());
        segment.included.push("bar".to_string());
        segment.included_contexts.push(SegmentTarget {
            values: vec![],
            context_kind: Kind::user(),
        });
        let context = ContextBuilder::new("bar").build().unwrap();
        assert_segment_match(&segment, context, true);
    }

    proptest! {
        #[test]
        fn user_is_matched_by_segment_rule(kind in of(Just(Kind::user()))) {
            let mut segment = new_segment();
            segment.rules.push(jane_rule(None, None, kind));
            let jane = ContextBuilder::new("foo").name("Jane").build().unwrap();
            let joan = ContextBuilder::new("foo").name("Joan").build().unwrap();
            assert_segment_match(&segment, jane, true);
            assert_segment_match(&segment, joan, false);
        }
    }

    proptest! {
        #[test]
        fn user_is_explicitly_excluded_from_segment(kind in of(Just(Kind::user()))) {
            let mut segment = new_segment();
            segment.rules.push(jane_rule(None, None, kind));
            segment.excluded.push("foo".to_string());
            segment.excluded.push("bar".to_string());
            segment.excluded_contexts.push(SegmentTarget {
                values: vec![],
                context_kind: Kind::user(),
            });
            let jane = ContextBuilder::new("foo").name("Jane").build().unwrap();
            assert_segment_match(&segment, jane, false);
        }
    }

    #[test]
    fn segment_includes_override_excludes() {
        let mut segment = new_segment();
        segment.included.push("bar".to_string());
        segment.included_contexts.push(SegmentTarget {
            values: vec![],
            context_kind: Kind::user(),
        });
        segment.excluded.push("foo".to_string());
        segment.excluded.push("bar".to_string());
        segment.excluded_contexts.push(SegmentTarget {
            values: vec![],
            context_kind: Kind::user(),
        });
        let context = ContextBuilder::new("bar").build().unwrap();
        assert_segment_match(&segment, context, true);
    }

    #[test]
    fn user_is_explicitly_included_in_context_match() {
        let mut segment = new_segment();
        segment.included_contexts.push(SegmentTarget {
            values: vec!["foo".to_string()],
            context_kind: Kind::user(),
        });
        segment.included_contexts.push(SegmentTarget {
            values: vec!["bar".to_string()],
            context_kind: Kind::user(),
        });
        let context = ContextBuilder::new("bar").build().unwrap();
        assert_segment_match(&segment, context, true);
    }

    #[test]
    fn segment_include_target_does_not_match_with_mismatched_context() {
        let mut segment = new_segment();
        segment.included_contexts.push(SegmentTarget {
            values: vec!["bar".to_string()],
            context_kind: Kind::from("org"),
        });
        let context = ContextBuilder::new("bar").build().unwrap();
        assert_segment_match(&segment, context, false);
    }

    proptest! {
        #[test]
        fn user_is_explicitly_excluded_in_context_match(kind in of(Just(Kind::user()))) {
            let mut segment = new_segment();
            segment.rules.push(jane_rule(None, None, kind));
            segment.excluded_contexts.push(SegmentTarget {
                values: vec!["foo".to_string()],
                context_kind: Kind::user(),
            });
            segment.excluded_contexts.push(SegmentTarget {
                values: vec!["bar".to_string()],
                context_kind: Kind::user(),
            });
            let jane = ContextBuilder::new("foo").name("Jane").build().unwrap();
            assert_segment_match(&segment, jane, false);
        }

        #[test]
        fn segment_does_not_match_if_no_includes_or_rules_match(kind in of(Just(Kind::user()))) {
            let mut segment = new_segment();
            segment.rules.push(jane_rule(None, None, kind));
            segment.included.push("key".to_string());
            let context = ContextBuilder::new("other-key")
                .name("Bob")
                .build()
                .unwrap();
            assert_segment_match(&segment, context, false);
        }

        #[test]
        fn segment_rule_can_match_user_with_percentage_rollout(kind in of(Just(Kind::user()))) {
            let mut segment = new_segment();
            segment.rules.push(jane_rule(Some(99_999.0), None, kind));
            let context = ContextBuilder::new("key").name("Jane").build().unwrap();
            assert_segment_match(&segment, context, true);
        }

        #[test]
        fn segment_rule_can_not_match_user_with_percentage_rollout(kind in of(Just(Kind::user()))) {
            let mut segment = new_segment();
            segment.rules.push(jane_rule(Some(1.0), None, kind));
            let context = ContextBuilder::new("key").name("Jane").build().unwrap();
            assert_segment_match(&segment, context, false);
        }

        #[test]
        fn segment_rule_can_have_percentage_rollout(kind in of(Just(Kind::user()))) {
            let mut segment = new_segment();
            segment.rules.push(thirty_percent_rule(None, kind));

            let context_a = ContextBuilder::new("userKeyA").build().unwrap(); // bucket 0.14574753
            let context_z = ContextBuilder::new("userKeyZ").build().unwrap(); // bucket 0.45679215
            assert_segment_match(&segment, context_a, true);
            assert_segment_match(&segment, context_z, false);
        }

        #[test]
        fn segment_rule_can_have_percentage_rollout_by_any_attribute(kind in of(Just(Kind::user()))) {
            let mut segment = new_segment();
            segment
                .rules
                .push(thirty_percent_rule(Some(Reference::new("name")), kind));
            let context_a = ContextBuilder::new("x").name("userKeyA").build().unwrap(); // bucket 0.14574753
            let context_z = ContextBuilder::new("x").name("userKeyZ").build().unwrap(); // bucket 0.45679215
            assert_segment_match(&segment, context_a, true);
            assert_segment_match(&segment, context_z, false);
        }
    }
}
