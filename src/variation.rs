use serde::{Deserialize, Serialize};

use crate::{user::User, BucketPrefix};

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

/// Rollout describes how users will be bucketed into variations during a percentage rollout.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Rollout {
    #[serde(default)]
    kind: RolloutKind,
    bucket_by: Option<String>,
    variations: Vec<WeightedVariation>,
    seed: Option<i64>,
}

impl Rollout {
    #[cfg(test)]
    fn with_variations<V: Into<Vec<WeightedVariation>>>(variations: V) -> Self {
        Rollout {
            kind: RolloutKind::Rollout,
            bucket_by: None,
            seed: None,
            variations: variations.into(),
        }
    }

    #[cfg(test)]
    fn bucket_by(self, bucket_by: &str) -> Self {
        Rollout {
            bucket_by: Some(bucket_by.into()),
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

/// WeightedVariation describes a fraction of users who will receive a specific variation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct WeightedVariation {
    /// The index of the variation to be returned if the user is in this bucket. This is always a
    /// real variation index; it cannot be undefined.
    pub variation: VariationIndex,

    /// The proportion of users who should go into this bucket, as an integer from 0 to 100000.
    pub weight: VariationWeight,

    /// Untracked means that users allocated to this variation should not have tracking events sent.
    #[serde(default)]
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
        user: &User,
        salt: &str,
    ) -> Option<BucketResult> {
        match self {
            VariationOrRollout::Variation { variation: var } => Some(var.into()),
            VariationOrRollout::Rollout {
                rollout:
                    Rollout {
                        kind,
                        bucket_by,
                        variations,
                        seed,
                    },
            } => {
                let is_experiment = kind == &RolloutKind::Experiment;

                let prefix = match seed {
                    Some(seed) => BucketPrefix::Seed(*seed),
                    None => BucketPrefix::KeyAndSalt(flag_key, salt),
                };

                let bucket = user.bucket(bucket_by.as_deref(), prefix);
                let mut sum = 0.0;
                for variation in variations {
                    sum += variation.weight / 100_000.0;
                    if bucket < sum {
                        return Some(variation.as_bucket_result(is_experiment));
                    }
                }
                variations
                    .last()
                    .map(|var| var.as_bucket_result(is_experiment))
            }
            VariationOrRollout::Malformed(_) => None,
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
    use crate::user::User;

    use super::*;
    use spectral::prelude::*;

    use maplit::hashmap;

    const BUCKET_TOLERANCE: f32 = 0.0000001;

    #[test]
    fn variation_index_for_user() {
        const HASH_KEY: &str = "hashKey";
        const SALT: &str = "saltyA";

        let wv0 = WeightedVariation::new(0, 60_000.0);
        let wv1 = WeightedVariation::new(1, 40_000.0);
        let rollout = VariationOrRollout::Rollout {
            rollout: Rollout::with_variations(vec![wv0, wv1]),
        };

        asserting!("userKeyA (bucket 0.42157587) should get variation 0")
            .that(&rollout.variation(HASH_KEY, &User::with_key("userKeyA").build(), SALT))
            .contains_value(BucketResult {
                variation_index: 0,
                in_experiment: false,
            });
        asserting!("userKeyB (bucket 0.6708485) should get variation 1")
            .that(&rollout.variation(HASH_KEY, &User::with_key("userKeyB").build(), SALT))
            .contains_value(BucketResult {
                variation_index: 1,
                in_experiment: false,
            });
        asserting!("userKeyC (bucket 0.10343106) should get variation 0")
            .that(&rollout.variation(HASH_KEY, &User::with_key("userKeyC").build(), SALT))
            .contains_value(BucketResult {
                variation_index: 0,
                in_experiment: false,
            });
    }

    #[test]
    fn variation_index_for_user_with_custom_attribute() {
        const HASH_KEY: &str = "hashKey";
        const SALT: &str = "saltyA";

        let wv0 = WeightedVariation::new(0, 60_000.0);
        let wv1 = WeightedVariation::new(1, 40_000.0);
        let rollout = VariationOrRollout::Rollout {
            rollout: Rollout::with_variations(vec![wv0, wv1]).bucket_by("intAttr"),
        };

        asserting!("userKeyD (bucket 0.54771423) should get variation 0")
            .that(
                &rollout.variation(
                    HASH_KEY,
                    &User::with_key("userKeyA")
                        .custom(hashmap! {"intAttr".into() => 33_333.into()})
                        .build(),
                    SALT,
                ),
            )
            .contains_value(BucketResult {
                variation_index: 0,
                in_experiment: false,
            });

        asserting!("userKeyD (bucket 0.7309658) should get variation 0")
            .that(
                &rollout.variation(
                    HASH_KEY,
                    &User::with_key("userKeyA")
                        .custom(hashmap! {"intAttr".into() => 99_999.into()})
                        .build(),
                    SALT,
                ),
            )
            .contains_value(BucketResult {
                variation_index: 1,
                in_experiment: false,
            });
    }

    #[test]
    fn variation_index_for_user_in_experiment() {
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
                kind: RolloutKind::Experiment,
                ..Rollout::with_variations(vec![wv0, wv1, wv0_untracked])
            },
        };

        asserting!("userKeyA (bucket 0.09801207) should get variation 0 and be in the experiment")
            .that(&rollout.variation(HASH_KEY, &User::with_key("userKeyA").build(), SALT))
            .contains_value(BucketResult {
                variation_index: 0,
                in_experiment: true,
            });
        asserting!("userKeyB (bucket 0.14483777) should get variation 1 and be in the experiment")
            .that(&rollout.variation(HASH_KEY, &User::with_key("userKeyB").build(), SALT))
            .contains_value(BucketResult {
                variation_index: 1,
                in_experiment: true,
            });
        asserting!(
            "userKeyC (bucket 0.9242641) should get variation 0 and not be in the experiment"
        )
        .that(&rollout.variation(HASH_KEY, &User::with_key("userKeyC").build(), SALT))
        .contains_value(BucketResult {
            variation_index: 0,
            in_experiment: false,
        });
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
    fn bucket_user_with_secondary_key() {
        const PREFIX: BucketPrefix = BucketPrefix::KeyAndSalt("hashKey", "salt");

        let u1 = User::with_key("userKey").build();
        let u2 = User::with_key("userKey")
            .secondary("mySecondaryKey")
            .build();
        let bucket1 = u1.bucket(None, PREFIX);
        let bucket2 = u2.bucket(None, PREFIX);
        assert_that!(bucket1).is_not_equal_to(bucket2);
    }

    #[test]
    fn bucket_user_by_int_attr() {
        const USER_KEY: &str = "userKeyD";
        const PREFIX: BucketPrefix = BucketPrefix::KeyAndSalt("hashKey", "saltyA");

        let custom = hashmap! {
            "intAttr".into() => 33_333.into(),
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
    fn bucket_user_by_float_attr_that_is_really_an_int_is_allowed() {
        const PREFIX: BucketPrefix = BucketPrefix::KeyAndSalt("hashKey", "saltyA");

        let custom = hashmap! {
            "floatAttr".into() => f64::from(33_333).into()
        };
        let user = User::with_key("userKeyE").custom(custom).build();
        let bucket = user.bucket(Some("floatAttr"), PREFIX);
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
                kind: RolloutKind::Rollout,
                ..Rollout::with_variations(vec![wv0, wv1])
            },
        };
        let custom = hashmap! {
            "intAttr".into() => 99_999.into()
        };
        let user = User::with_key("userKeyD").custom(custom).build();
        asserting!("userKeyD should get variation 1 and not be in the experiment")
            .that(&rollout.variation(HASH_KEY, &user, SALT))
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
                kind: RolloutKind::Experiment,
                ..Rollout::with_variations(vec![wv0, wv1])
            },
        };
        let custom = hashmap! {
            "intAttr".into() => 99_999.into()
        };
        let user = User::with_key("userKeyD").custom(custom).build();
        asserting!("userKeyD should get variation 1 and be in the experiment")
            .that(&rollout.variation(HASH_KEY, &user, SALT))
            .contains_value(BucketResult {
                variation_index: 1,
                in_experiment: true,
            });
    }
}

#[cfg(test)]
mod tests {
    use crate::user::User;

    use super::*;
    use spectral::prelude::*;

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
                bucket_by: Some("bucket".to_string()),
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
                kind: RolloutKind::Experiment,
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
        assert_that!(malformed).is_equal_to(VariationOrRollout::Malformed(serde_json::json!({})));

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
            .that(&rollout.variation(HASH_KEY, &User::with_key("userKeyD").build(), SALT))
            .contains_value(BucketResult {
                variation_index: 2,
                in_experiment: false,
            });
    }
}
