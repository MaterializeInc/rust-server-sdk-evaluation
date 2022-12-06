use serde::{Deserialize, Serialize};

use crate::contexts::attribute_reference::AttributeName;
use crate::util::is_false;
use crate::{
    contexts::context::{BucketPrefix, Kind},
    Context, Reference,
};
use serde_with::skip_serializing_none;

/// A type representing the index into the [crate::Flag]'s variations.
pub type VariationIndex = isize;

#[derive(Debug, PartialEq)]
pub(crate) struct BucketResult {
    pub variation_index: VariationIndex,
    pub in_experiment: bool,
}

impl From<&VariationIndex> for BucketResult {
    fn from(variation_index: &VariationIndex) -> Self {
        BucketResult {
            variation_index: *variation_index,
            in_experiment: false, // single variations are never in an experiment
        }
    }
}

/// RolloutKind describes whether a rollout is a simple percentage rollout or represents an
/// experiment. Experiments have different behaviour for tracking and variation bucketing.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum RolloutKind {
    /// Represents a simple percentage rollout. This is the default rollout kind, and will be assumed if
    /// not otherwise specified.
    Rollout,

    /// Represents an experiment. Experiments have different behaviour for tracking and variation
    /// bucketing.
    Experiment,
}

impl Default for RolloutKind {
    fn default() -> Self {
        RolloutKind::Rollout
    }
}

/// Rollout describes how contexts will be bucketed into variations during a percentage rollout.
// Rollout is deserialized via a helper, IntermediateRollout, because of semantic ambiguity
// of the bucketBy Reference field.
//
// Rollout implements Serialize directly without a helper because References can serialize
// themselves without any ambiguity.
#[skip_serializing_none]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", from = "IntermediateRollout")]
pub struct Rollout {
    // Specifies if this rollout is a simple rollout or an experiment. Should default to rollout
    // if absent.
    kind: Option<RolloutKind>,
    // Context kind associated with this rollout.
    context_kind: Option<Kind>,
    // Specifies which attribute should be used to distinguish between Contexts in a rollout.
    // Can be omitted; evaluation should treat absence as 'key'.
    bucket_by: Option<Reference>,
    // Which variations should be included in the rollout, and associated weight.
    variations: Vec<WeightedVariation>,
    // Specifies the seed to be used by the hashing algorithm.
    seed: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RolloutWithContextKind {
    kind: Option<RolloutKind>,
    context_kind: Kind,
    // bucketBy deserializes directly into a reference.
    bucket_by: Option<Reference>,
    variations: Vec<WeightedVariation>,
    seed: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RolloutWithoutContextKind {
    kind: Option<RolloutKind>,
    bucket_by: Option<AttributeName>,
    variations: Vec<WeightedVariation>,
    seed: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum IntermediateRollout {
    // RolloutWithContextKind must be listed first in the enum because otherwise
    // RolloutWithoutContextKind could match the input (by ignoring/discarding
    // the context_kind field).
    ContextAware(RolloutWithContextKind),
    ContextOblivious(RolloutWithoutContextKind),
}

impl From<IntermediateRollout> for Rollout {
    fn from(rollout: IntermediateRollout) -> Self {
        match rollout {
            IntermediateRollout::ContextAware(fields) => Self {
                kind: fields.kind,
                context_kind: Some(fields.context_kind),
                bucket_by: fields.bucket_by,
                variations: fields.variations,
                seed: fields.seed,
            },
            IntermediateRollout::ContextOblivious(fields) => Self {
                kind: fields.kind,
                context_kind: None,
                bucket_by: fields.bucket_by.map(Reference::from),
                variations: fields.variations,
                seed: fields.seed,
            },
        }
    }
}

#[cfg(test)]
pub(crate) mod proptest_generators {
    use super::*;
    use crate::proptest_generators::{any_kind, any_ref};
    use proptest::{collection::vec, option::of, prelude::*};

    prop_compose! {
        fn any_weighted_variation() (
            variation in any::<isize>(),
            weight in any::<f32>(),
            untracked in any::<bool>()
        ) -> WeightedVariation {
            WeightedVariation {
                variation,
                weight,
                untracked
            }
        }
    }

    fn any_rollout_kind() -> BoxedStrategy<RolloutKind> {
        prop_oneof![Just(RolloutKind::Rollout), Just(RolloutKind::Experiment)].boxed()
    }

    prop_compose! {
        pub(crate) fn any_rollout() (
            kind in of(any_rollout_kind()),
            context_kind in any_kind(),
            bucket_by in of(any_ref()),
            seed in of(any::<i64>()),
            variations in vec(any_weighted_variation(), 0..2)
        ) -> Rollout {
            Rollout {
                kind,
                context_kind: Some(context_kind),
                bucket_by,
                seed,
                variations,
            }
        }
    }
}

impl Rollout {
    #[cfg(test)]
    fn with_variations<V: Into<Vec<WeightedVariation>>>(variations: V) -> Self {
        Rollout {
            kind: None,
            context_kind: None,
            bucket_by: None,
            seed: None,
            variations: variations.into(),
        }
    }

    #[cfg(test)]
    fn bucket_by(self, bucket_by: &str) -> Self {
        Rollout {
            bucket_by: Some(Reference::new(bucket_by)),
            ..self
        }
    }
}

/// VariationOrRollout describes either a fixed variation or a percentage rollout.
///
/// There is a VariationOrRollout for every [crate::FlagRule], and also one in [crate::eval::Reason::Fallthrough] which is
/// used if no rules match.
///
/// Invariant: one of the variation or rollout must be non-nil.
// This enum is a bit oddly-shaped because data errors may cause the server to emit rules with neither or both of a
// variation or rollout, and we need to treat invalid states with grace (i.e. don't throw a 500 on deserialization, and
// prefer variation if both are present)
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum VariationOrRollout {
    /// Represents a fixed variation.
    Variation {
        /// The index of the variation to return. It is undefined if no specific variation is defined.
        variation: VariationIndex,
    },
    /// Represents a percentage rollout.
    Rollout {
        /// Specifies the rollout definition. See [Rollout].
        rollout: Rollout,
    },
    /// Represents a malformed VariationOrRollout payload. This is done to deal with potential
    /// data errors that may occur server-side. Generally speaking this should not occur.
    Malformed(serde_json::Value),
}

pub(crate) type VariationWeight = f32;

/// WeightedVariation describes a fraction of contexts which will receive a specific variation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct WeightedVariation {
    /// The index of the variation to be returned if the context is in this bucket. This is always a
    /// real variation index; it cannot be undefined.
    pub variation: VariationIndex,

    /// The proportion of contexts which should go into this bucket, as an integer from 0 to 100000.
    pub weight: VariationWeight,

    /// Untracked means that contexts allocated to this variation should not have tracking events sent.
    #[serde(default, skip_serializing_if = "is_false")]
    pub untracked: bool,
}

impl WeightedVariation {
    #[cfg(test)]
    fn new(variation: VariationIndex, weight: VariationWeight) -> Self {
        WeightedVariation {
            variation,
            weight,
            untracked: false,
        }
    }

    fn as_bucket_result(&self, is_experiment: bool) -> BucketResult {
        BucketResult {
            variation_index: self.variation,
            in_experiment: is_experiment && !self.untracked,
        }
    }
}

impl VariationOrRollout {
    pub(crate) fn variation(
        &self,
        flag_key: &str,
        context: &Context,
        salt: &str,
    ) -> Result<Option<BucketResult>, String> {
        match self {
            VariationOrRollout::Variation { variation: var } => Ok(Some(var.into())),
            VariationOrRollout::Rollout {
                rollout:
                    Rollout {
                        kind,
                        bucket_by,
                        variations,
                        seed,
                        context_kind,
                    },
            } => {
                let is_experiment =
                    kind.as_ref().unwrap_or(&RolloutKind::default()) == &RolloutKind::Experiment;

                let prefix = match seed {
                    Some(seed) => BucketPrefix::Seed(*seed),
                    None => BucketPrefix::KeyAndSalt(flag_key, salt),
                };

                let (bucket, was_missing_context) = context.bucket(
                    bucket_by,
                    prefix,
                    is_experiment,
                    context_kind.as_ref().unwrap_or(&Kind::default()),
                )?;

                let mut sum = 0.0;
                for variation in variations {
                    sum += variation.weight / 100_000.0;
                    if bucket < sum {
                        return Ok(Some(
                            variation.as_bucket_result(is_experiment && !was_missing_context),
                        ));
                    }
                }
                return Ok(variations
                    .last()
                    .map(|var| var.as_bucket_result(is_experiment && !was_missing_context)));
            }
            VariationOrRollout::Malformed(_) => Ok(None),
        }
    }
}

// Note: These tests are meant to be exact duplicates of tests
// in other SDKs. Do not change any of the values unless they
// are also changed in other SDKs. These are not traditional behavioral
// tests so much as consistency tests to guarantee that the implementation
// is identical across SDKs.

#[cfg(test)]
mod consistency_tests {
    use super::*;
    use crate::ContextBuilder;
    use serde_json::json;
    use spectral::prelude::*;
    use test_case::test_case;

    const BUCKET_TOLERANCE: f32 = 0.0000001;

    #[test]
    fn variation_index_for_context() {
        const HASH_KEY: &str = "hashKey";
        const SALT: &str = "saltyA";

        let wv0 = WeightedVariation::new(0, 60_000.0);
        let wv1 = WeightedVariation::new(1, 40_000.0);
        let rollout = VariationOrRollout::Rollout {
            rollout: Rollout::with_variations(vec![wv0, wv1]),
        };

        asserting!("userKeyA (bucket 0.42157587) should get variation 0")
            .that(
                &rollout
                    .variation(
                        HASH_KEY,
                        &ContextBuilder::new("userKeyA").build().unwrap(),
                        SALT,
                    )
                    .unwrap(),
            )
            .contains_value(BucketResult {
                variation_index: 0,
                in_experiment: false,
            });
        asserting!("userKeyB (bucket 0.6708485) should get variation 1")
            .that(
                &rollout
                    .variation(
                        HASH_KEY,
                        &ContextBuilder::new("userKeyB").build().unwrap(),
                        SALT,
                    )
                    .unwrap(),
            )
            .contains_value(BucketResult {
                variation_index: 1,
                in_experiment: false,
            });
        asserting!("userKeyC (bucket 0.10343106) should get variation 0")
            .that(
                &rollout
                    .variation(
                        HASH_KEY,
                        &ContextBuilder::new("userKeyC").build().unwrap(),
                        SALT,
                    )
                    .unwrap(),
            )
            .contains_value(BucketResult {
                variation_index: 0,
                in_experiment: false,
            });
    }

    #[test_case(None, "userKeyA", 2)] // 0.42157587,
    #[test_case(None, "userKeyB", 2)] // 0.6708485,
    #[test_case(None, "userKeyC", 1)] // 0.10343106,
    #[test_case(Some(61), "userKeyA", 0)] // 0.09801207,
    #[test_case(Some(61), "userKeyB", 1)] // 0.14483777,
    #[test_case(Some(61), "userKeyC", 2)] // 0.9242641,
    fn testing_experiment_bucketing(
        seed: Option<i64>,
        key: &str,
        expected_variation_index: VariationIndex,
    ) {
        const HASH_KEY: &str = "hashKey";
        const SALT: &str = "saltyA";

        let wv0 = WeightedVariation::new(0, 10_000.0);
        let wv1 = WeightedVariation::new(1, 20_000.0);
        let wv2 = WeightedVariation::new(2, 70_000.0);

        let mut rollout = Rollout::with_variations(vec![wv0, wv1, wv2]).bucket_by("intAttr");
        rollout.kind = Some(RolloutKind::Experiment);
        rollout.seed = seed;
        let rollout = VariationOrRollout::Rollout { rollout };

        let result = rollout
            .variation(
                HASH_KEY,
                &ContextBuilder::new(key)
                    .set_value("intAttr", 0.6708485.into())
                    .build()
                    .unwrap(),
                SALT,
            )
            .unwrap()
            .unwrap();

        assert_eq!(result.variation_index, expected_variation_index);
    }

    #[test]
    fn variation_index_for_context_with_custom_attribute() {
        const HASH_KEY: &str = "hashKey";
        const SALT: &str = "saltyA";

        let wv0 = WeightedVariation::new(0, 60_000.0);
        let wv1 = WeightedVariation::new(1, 40_000.0);
        let rollout = VariationOrRollout::Rollout {
            rollout: Rollout::with_variations(vec![wv0, wv1]).bucket_by("intAttr"),
        };

        asserting!("userKeyD (bucket 0.54771423) should get variation 0")
            .that(
                &rollout
                    .variation(
                        HASH_KEY,
                        &ContextBuilder::new("userKeyA")
                            .set_value("intAttr", 33_333.into())
                            .build()
                            .unwrap(),
                        SALT,
                    )
                    .unwrap(),
            )
            .contains_value(BucketResult {
                variation_index: 0,
                in_experiment: false,
            });

        asserting!("userKeyD (bucket 0.7309658) should get variation 0")
            .that(
                &rollout
                    .variation(
                        HASH_KEY,
                        &ContextBuilder::new("userKeyA")
                            .set_value("intAttr", 99_999.into())
                            .build()
                            .unwrap(),
                        SALT,
                    )
                    .unwrap(),
            )
            .contains_value(BucketResult {
                variation_index: 1,
                in_experiment: false,
            });
    }

    #[test]
    fn variation_index_for_context_in_experiment() {
        const HASH_KEY: &str = "hashKey";
        const SALT: &str = "saltyA";

        let wv0 = WeightedVariation {
            variation: 0,
            weight: 10_000.0,
            untracked: false,
        };
        let wv1 = WeightedVariation {
            variation: 1,
            weight: 20_000.0,
            untracked: false,
        };
        let wv0_untracked = WeightedVariation {
            variation: 0,
            weight: 70_000.0,
            untracked: true,
        };
        let rollout = VariationOrRollout::Rollout {
            rollout: Rollout {
                seed: Some(61),
                kind: Some(RolloutKind::Experiment),
                ..Rollout::with_variations(vec![wv0, wv1, wv0_untracked])
            },
        };

        asserting!("userKeyA (bucket 0.09801207) should get variation 0 and be in the experiment")
            .that(
                &rollout
                    .variation(
                        HASH_KEY,
                        &ContextBuilder::new("userKeyA").build().unwrap(),
                        SALT,
                    )
                    .unwrap(),
            )
            .contains_value(BucketResult {
                variation_index: 0,
                in_experiment: true,
            });
        asserting!("userKeyB (bucket 0.14483777) should get variation 1 and be in the experiment")
            .that(
                &rollout
                    .variation(
                        HASH_KEY,
                        &ContextBuilder::new("userKeyB").build().unwrap(),
                        SALT,
                    )
                    .unwrap(),
            )
            .contains_value(BucketResult {
                variation_index: 1,
                in_experiment: true,
            });
        asserting!(
            "userKeyC (bucket 0.9242641) should get variation 0 and not be in the experiment"
        )
        .that(
            &rollout
                .variation(
                    HASH_KEY,
                    &ContextBuilder::new("userKeyC").build().unwrap(),
                    SALT,
                )
                .unwrap(),
        )
        .contains_value(BucketResult {
            variation_index: 0,
            in_experiment: false,
        });
    }

    #[test]
    fn bucket_context_by_key() {
        const PREFIX: BucketPrefix = BucketPrefix::KeyAndSalt("hashKey", "saltyA");

        let context = ContextBuilder::new("userKeyA").build().unwrap();
        let (bucket, _) = context.bucket(&None, PREFIX, false, &Kind::user()).unwrap();
        assert_that!(bucket).is_close_to(0.42157587, BUCKET_TOLERANCE);

        let context = ContextBuilder::new("userKeyB").build().unwrap();
        let (bucket, _) = context.bucket(&None, PREFIX, false, &Kind::user()).unwrap();
        assert_that!(bucket).is_close_to(0.6708485, BUCKET_TOLERANCE);

        let context = ContextBuilder::new("userKeyC").build().unwrap();
        let (bucket, _) = context.bucket(&None, PREFIX, false, &Kind::user()).unwrap();
        assert_that!(bucket).is_close_to(0.10343106, BUCKET_TOLERANCE);

        let result = context.bucket(&Some(Reference::new("")), PREFIX, false, &Kind::user());
        assert!(result.is_err());
    }

    #[test]
    fn bucket_context_by_key_with_seed() {
        const PREFIX: BucketPrefix = BucketPrefix::Seed(61);

        let context_a = ContextBuilder::new("userKeyA").build().unwrap();
        let (bucket, _) = context_a
            .bucket(&None, PREFIX, false, &Kind::user())
            .unwrap();
        assert_that!(bucket).is_close_to(0.09801207, BUCKET_TOLERANCE);

        let context_b = ContextBuilder::new("userKeyB").build().unwrap();
        let (bucket, _) = context_b
            .bucket(&None, PREFIX, false, &Kind::user())
            .unwrap();
        assert_that!(bucket).is_close_to(0.14483777, BUCKET_TOLERANCE);

        let context_c = ContextBuilder::new("userKeyC").build().unwrap();
        let (bucket, _) = context_c
            .bucket(&None, PREFIX, false, &Kind::user())
            .unwrap();
        assert_that!(bucket).is_close_to(0.9242641, BUCKET_TOLERANCE);

        // changing seed produces different bucket value
        let (bucket, _) = context_a
            .bucket(&None, BucketPrefix::Seed(60), false, &Kind::user())
            .unwrap();
        assert_that!(bucket).is_close_to(0.7008816, BUCKET_TOLERANCE)
    }

    #[test]
    #[cfg_attr(not(feature = "secondary_key_bucketing"), ignore)]
    fn bucket_context_with_secondary_key_only_when_feature_enabled() {
        const PREFIX: BucketPrefix = BucketPrefix::KeyAndSalt("hashKey", "salt");

        let context1 = ContextBuilder::new("userKey").build().unwrap();

        // can only construct a context w/ secondary by deserializing from implicit user format.
        let context2: Context = serde_json::from_value(json!({
            "key" : "userKey",
            "secondary" : "mySecondaryKey"
        }))
        .unwrap();

        let result1 = context1.bucket(&None, PREFIX, false, &Kind::user());
        let result2 = context2.bucket(&None, PREFIX, false, &Kind::user());
        assert_that!(result1).is_not_equal_to(result2);
    }

    #[test]
    #[cfg_attr(feature = "secondary_key_bucketing", ignore)]
    fn bucket_context_with_secondary_key_does_not_change_result() {
        const PREFIX: BucketPrefix = BucketPrefix::KeyAndSalt("hashKey", "salt");

        let context1: Context = ContextBuilder::new("userKey").build().unwrap();

        // can only construct a context w/ secondary by deserializing from implicit user format.
        let context2: Context = serde_json::from_value(json!({
            "key" : "userKey",
            "secondary" : "mySecondaryKey"
        }))
        .unwrap();

        let result1 = context1.bucket(&None, PREFIX, false, &Kind::user());
        let result2 = context2.bucket(&None, PREFIX, false, &Kind::user());
        assert_that!(result1).is_equal_to(result2);
    }

    #[test]
    fn bucket_context_by_int_attr() {
        const USER_KEY: &str = "userKeyD";
        const PREFIX: BucketPrefix = BucketPrefix::KeyAndSalt("hashKey", "saltyA");

        let context = ContextBuilder::new(USER_KEY)
            .set_value("intAttr", 33_333.into())
            .build()
            .unwrap();
        let (bucket, _) = context
            .bucket(
                &Some(Reference::new("intAttr")),
                PREFIX,
                false,
                &Kind::user(),
            )
            .unwrap();
        assert_that!(bucket).is_close_to(0.54771423, BUCKET_TOLERANCE);

        let context = ContextBuilder::new(USER_KEY)
            .set_value("stringAttr", "33333".into())
            .build()
            .unwrap();
        let (bucket2, _) = context
            .bucket(
                &Some(Reference::new("stringAttr")),
                PREFIX,
                false,
                &Kind::user(),
            )
            .unwrap();
        assert_that!(bucket).is_close_to(bucket2, BUCKET_TOLERANCE);
    }

    #[test]
    fn bucket_context_by_float_attr_not_allowed() {
        const USER_KEY: &str = "userKeyE";
        const PREFIX: BucketPrefix = BucketPrefix::KeyAndSalt("hashKey", "saltyA");

        let context = ContextBuilder::new(USER_KEY)
            .set_value("floatAttr", 999.999.into())
            .build()
            .unwrap();
        let (bucket, _) = context
            .bucket(
                &Some(Reference::new("floatAttr")),
                PREFIX,
                false,
                &Kind::user(),
            )
            .unwrap();
        assert_that!(bucket).is_close_to(0.0, BUCKET_TOLERANCE);
    }

    #[test]
    fn bucket_context_by_float_attr_that_is_really_an_int_is_allowed() {
        const PREFIX: BucketPrefix = BucketPrefix::KeyAndSalt("hashKey", "saltyA");

        let context = ContextBuilder::new("userKeyE")
            .set_value("floatAttr", f64::from(33_333).into())
            .build()
            .unwrap();
        let (bucket, _) = context
            .bucket(
                &Some(Reference::new("floatAttr")),
                PREFIX,
                false,
                &Kind::user(),
            )
            .unwrap();
        assert_that!(bucket).is_close_to(0.54771423, BUCKET_TOLERANCE);
    }

    #[test]
    fn test_bucket_value_beyond_last_bucket_is_pinned_to_last_bucket() {
        const HASH_KEY: &str = "hashKey";
        const SALT: &str = "saltyA";

        let wv0 = WeightedVariation::new(0, 5_000.0);
        let wv1 = WeightedVariation::new(1, 5_000.0);
        let rollout = VariationOrRollout::Rollout {
            rollout: Rollout {
                seed: Some(61),
                kind: Some(RolloutKind::Rollout),
                ..Rollout::with_variations(vec![wv0, wv1])
            },
        };
        let context = ContextBuilder::new("userKeyD")
            .set_value("intAttr", 99_999.into())
            .build()
            .unwrap();
        asserting!("userKeyD should get variation 1 and not be in the experiment")
            .that(&rollout.variation(HASH_KEY, &context, SALT).unwrap())
            .contains_value(BucketResult {
                variation_index: 1,
                in_experiment: false,
            });
    }

    #[test]
    fn test_bucket_value_beyond_last_bucket_is_pinned_to_last_bucket_for_experiment() {
        const HASH_KEY: &str = "hashKey";
        const SALT: &str = "saltyA";

        let wv0 = WeightedVariation::new(0, 5_000.0);
        let wv1 = WeightedVariation::new(1, 5_000.0);
        let rollout = VariationOrRollout::Rollout {
            rollout: Rollout {
                seed: Some(61),
                kind: Some(RolloutKind::Experiment),
                ..Rollout::with_variations(vec![wv0, wv1])
            },
        };
        let context = ContextBuilder::new("userKeyD")
            .set_value("intAttr", 99_999.into())
            .build()
            .unwrap();
        asserting!("userKeyD should get variation 1 and be in the experiment")
            .that(&rollout.variation(HASH_KEY, &context, SALT).unwrap())
            .contains_value(BucketResult {
                variation_index: 1,
                in_experiment: true,
            });
    }
}

#[cfg(test)]
mod tests {
    use crate::ContextBuilder;
    use assert_json_diff::assert_json_eq;

    use super::*;
    use crate::proptest_generators::*;
    use proptest::prelude::*;
    use serde_json::json;
    use spectral::prelude::*;

    proptest! {
        #[test]
         fn arbitrary_rollout_serialization_roundtrip(rollout in any_rollout()) {
            let json = serde_json::to_value(&rollout).expect("a rollout should serialize");
            let parsed: Rollout = serde_json::from_value(json.clone()).expect("a rollout should parse");
            assert_json_eq!(json, parsed);
        }
    }

    #[test]
    fn rollout_serialize_omits_optional_fields() {
        let json = json!({"variations" : []});
        let parsed: Rollout = serde_json::from_value(json.clone()).expect("should parse");
        assert_json_eq!(json, parsed);
    }

    #[test]
    fn test_parse_variation_or_rollout() {
        let variation: VariationOrRollout =
            serde_json::from_str(r#"{"variation":4}"#).expect("should parse");

        assert_that!(variation).is_equal_to(&VariationOrRollout::Variation { variation: 4 });

        let rollout: VariationOrRollout =
            serde_json::from_str(r#"{"rollout":{"variations":[{"variation":1,"weight":100000}]}}"#)
                .expect("should parse");
        assert_that!(rollout).is_equal_to(&VariationOrRollout::Rollout {
            rollout: Rollout::with_variations(vec![WeightedVariation::new(1, 100_000.0)]),
        });

        let rollout_bucket_by: VariationOrRollout = serde_json::from_str(
            r#"{"rollout":{"bucketBy":"bucket","variations":[{"variation":1,"weight":100000}]}}"#,
        )
        .expect("should parse");
        assert_that!(rollout_bucket_by).is_equal_to(&VariationOrRollout::Rollout {
            rollout: Rollout {
                bucket_by: Some(Reference::new("bucket")),
                ..Rollout::with_variations(vec![WeightedVariation::new(1, 100_000.0)])
            },
        });

        let rollout_seed: VariationOrRollout = serde_json::from_str(
            r#"{"rollout":{"variations":[{"variation":1,"weight":100000}],"seed":42}}"#,
        )
        .expect("should parse");
        assert_that!(rollout_seed).is_equal_to(&VariationOrRollout::Rollout {
            rollout: Rollout {
                seed: Some(42),
                ..Rollout::with_variations(vec![WeightedVariation::new(1, 100_000.0)])
            },
        });

        let rollout_experiment: VariationOrRollout = serde_json::from_str(
            r#"{
                 "rollout":
                   {
                     "kind": "experiment",
                     "variations": [
                       {"variation":1, "weight":20000},
                       {"variation":0, "weight":20000},
                       {"variation":0, "weight":60000, "untracked": true}
                     ],
                     "seed":42
                   }
            }"#,
        )
        .expect("should parse");
        assert_that!(rollout_experiment).is_equal_to(&VariationOrRollout::Rollout {
            rollout: Rollout {
                kind: Some(RolloutKind::Experiment),
                seed: Some(42),
                ..Rollout::with_variations(vec![
                    WeightedVariation::new(1, 20_000.0),
                    WeightedVariation::new(0, 20_000.0),
                    WeightedVariation {
                        untracked: true,
                        ..WeightedVariation::new(0, 60_000.0)
                    },
                ])
            },
        });

        let malformed: VariationOrRollout = serde_json::from_str(r#"{}"#).expect("should parse");
        assert_that!(malformed).is_equal_to(VariationOrRollout::Malformed(json!({})));

        let overspecified: VariationOrRollout = serde_json::from_str(
            r#"{
                "variation": 1,
                "rollout": {"variations": [{"variation": 1, "weight": 100000}], "seed": 42}
            }"#,
        )
        .expect("should parse");
        assert_that!(overspecified).is_equal_to(VariationOrRollout::Variation { variation: 1 });
    }

    #[test]
    fn incomplete_weighting_defaults_to_last_variation() {
        const HASH_KEY: &str = "hashKey";
        const SALT: &str = "saltyA";

        let wv0 = WeightedVariation::new(0, 1.0);
        let wv1 = WeightedVariation::new(1, 2.0);
        let wv2 = WeightedVariation::new(2, 3.0);
        let rollout = VariationOrRollout::Rollout {
            rollout: Rollout::with_variations(vec![wv0, wv1, wv2]),
        };

        asserting!("userKeyD (bucket 0.7816281) should get variation 2")
            .that(
                &rollout
                    .variation(
                        HASH_KEY,
                        &ContextBuilder::new("userKeyD").build().unwrap(),
                        SALT,
                    )
                    .unwrap(),
            )
            .contains_value(BucketResult {
                variation_index: 2,
                in_experiment: false,
            });
    }
}
