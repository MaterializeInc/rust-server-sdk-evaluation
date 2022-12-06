use std::collections::HashSet;

use crate::flag::Flag;
use crate::flag_value::FlagValue;
use crate::store::Store;
use crate::variation::VariationIndex;
use crate::{BucketResult, Context, Target};
use log::warn;
use serde::Serialize;

/// A struct representing the results of an evaluation on a prerequisite flag.
pub struct PrerequisiteEvent {
    /// String representing the [crate::Flag::key] of the original flag being evaluated.
    pub target_flag_key: String,
    /// The [crate::Context] provided during the evaluation process.
    pub context: Context,
    /// The prerequisite [crate::Flag] that was evaluated.
    pub prerequisite_flag: Flag,
    /// The result of calling [evaluate] on the [PrerequisiteEvent::prerequisite_flag].
    pub prerequisite_result: Detail<FlagValue>,
}

/// Trait used by [evaluate] to record the result of prerequisite flag evaluations.
pub trait PrerequisiteEventRecorder {
    /// Record the results of a prerequisite flag evaluation.
    fn record(&self, event: PrerequisiteEvent);
}

const PREALLOCATED_PREREQUISITE_CHAIN_SIZE: usize = 20;
const PREALLOCATED_SEGMENT_CHAIN_SIZE: usize = 20;

pub(crate) struct EvaluationStack {
    pub(crate) prerequisite_flag_chain: HashSet<String>,
    pub(crate) segment_chain: HashSet<String>,
}

impl EvaluationStack {
    fn new() -> Self {
        // Preallocate some space for prerequisite_flag_chain and segment_chain on the stack. We
        // can get up to that many levels of nested prerequisites or nested segments before
        // appending to the slice will cause a heap allocation.
        Self {
            prerequisite_flag_chain: HashSet::with_capacity(PREALLOCATED_PREREQUISITE_CHAIN_SIZE),
            segment_chain: HashSet::with_capacity(PREALLOCATED_SEGMENT_CHAIN_SIZE),
        }
    }
}

impl Default for EvaluationStack {
    fn default() -> Self {
        Self::new()
    }
}

/// Evaluate a feature flag for the specified [Context].
///
/// The evaluator does not know anything about analytics events; generating any appropriate
/// analytics events is the responsibility of the caller. The caller can provide an optional
/// [PrerequisiteEventRecorder] which will be notified if any additional evaluations were done due
/// to prerequisites.
pub fn evaluate<'a>(
    store: &'a dyn Store,
    flag: &'a Flag,
    context: &'a Context,
    prerequisite_event_recorder: Option<&dyn PrerequisiteEventRecorder>,
) -> Detail<&'a FlagValue> {
    let mut evaluation_stack = EvaluationStack::default();
    evaluate_internal(
        store,
        flag,
        context,
        prerequisite_event_recorder,
        &mut evaluation_stack,
    )
}

fn evaluate_internal<'a>(
    store: &'a dyn Store,
    flag: &'a Flag,
    context: &'a Context,
    prerequisite_event_recorder: Option<&dyn PrerequisiteEventRecorder>,
    evaluation_stack: &mut EvaluationStack,
) -> Detail<&'a FlagValue> {
    if !flag.on {
        return flag.off_value(Reason::Off);
    }

    if evaluation_stack.prerequisite_flag_chain.contains(&flag.key) {
        warn!("prerequisite relationship to {} caused a circular reference; this is probably a temporary condition due to an incomplete update", flag.key);
        return flag.off_value(Reason::Error {
            error: Error::MalformedFlag,
        });
    }

    evaluation_stack
        .prerequisite_flag_chain
        .insert(flag.key.clone());

    for prereq in &flag.prerequisites {
        if let Some(prereq_flag) = store.flag(&prereq.key) {
            if evaluation_stack
                .prerequisite_flag_chain
                .contains(&prereq_flag.key)
            {
                return Detail::err(Error::MalformedFlag);
            }

            let prerequisite_result = evaluate_internal(
                store,
                &prereq_flag,
                context,
                prerequisite_event_recorder,
                evaluation_stack,
            );

            if let Detail {
                reason: Reason::Error { .. },
                ..
            } = prerequisite_result
            {
                return Detail::err(Error::MalformedFlag);
            }

            let variation_index = prerequisite_result.variation_index;

            if let Some(recorder) = prerequisite_event_recorder {
                recorder.record(PrerequisiteEvent {
                    target_flag_key: flag.key.clone(),
                    context: context.clone(),
                    prerequisite_flag: prereq_flag.clone(),
                    prerequisite_result: prerequisite_result.map(|v| v.clone()),
                });
            }

            if !prereq_flag.on || variation_index != Some(prereq.variation) {
                return flag.off_value(Reason::PrerequisiteFailed {
                    prerequisite_key: prereq.key.to_string(),
                });
            }
        } else {
            return flag.off_value(Reason::PrerequisiteFailed {
                prerequisite_key: prereq.key.to_string(),
            });
        }
    }

    evaluation_stack.prerequisite_flag_chain.remove(&flag.key);

    if let Some(variation_index) = any_target_match_variation(context, flag) {
        return flag.variation(variation_index, Reason::TargetMatch);
    }

    for (rule_index, rule) in flag.rules.iter().enumerate() {
        match rule.matches(context, store, evaluation_stack) {
            Err(e) => {
                warn!("{:?}", e);
                return Detail::err(Error::MalformedFlag);
            }
            Ok(matches) if matches => {
                let result = flag.resolve_variation_or_rollout(&rule.variation_or_rollout, context);
                return match result {
                    Ok(BucketResult {
                        variation_index,
                        in_experiment,
                    }) => {
                        let reason = Reason::RuleMatch {
                            rule_index,
                            rule_id: rule.id.clone(),
                            in_experiment,
                        };
                        flag.variation(variation_index, reason)
                    }
                    Err(e) => Detail::err(e),
                };
            }
            _ => (),
        }
    }

    let result = flag.resolve_variation_or_rollout(&flag.fallthrough, context);
    return match result {
        Ok(BucketResult {
            variation_index,
            in_experiment,
        }) => {
            let reason = Reason::Fallthrough { in_experiment };
            flag.variation(variation_index, reason)
        }
        Err(e) => Detail::err(e),
    };
}

fn any_target_match_variation(context: &Context, flag: &Flag) -> Option<VariationIndex> {
    if flag.context_targets.is_empty() {
        for target in &flag.targets {
            if let Some(index) = target_match_variation(context, target) {
                return Some(index);
            }
        }
    } else {
        for context_target in &flag.context_targets {
            if context_target.context_kind.is_user() && context_target.values.is_empty() {
                for target in &flag.targets {
                    if target.variation == context_target.variation {
                        if let Some(index) = target_match_variation(context, target) {
                            return Some(index);
                        }
                    }
                }
            } else if let Some(index) = target_match_variation(context, context_target) {
                return Some(index);
            }
        }
    }

    None
}

fn target_match_variation(context: &Context, target: &Target) -> Option<VariationIndex> {
    if let Some(context) = context.as_kind(&target.context_kind) {
        let key = context.key();
        for value in &target.values {
            if value == key {
                return Some(target.variation);
            }
        }
    }

    None
}

/// A Detail instance is returned from [evaluate], combining the result of a flag evaluation with
/// an explanation of how it was calculated.
#[derive(Clone, Debug, PartialEq)]
pub struct Detail<T> {
    /// The result of the flag evaluation. This will be either one of the flag's variations or None
    /// if no appropriate fallback value was configured.
    pub value: Option<T>,

    /// The index of the returned value within the flag's list of variations, e.g. 0 for the first
    /// variation. This is an Option because it is possible for the value to be undefined (there is
    /// no variation index if the application default value was returned due to an error in
    /// evaluation) which is different from a value of 0.
    pub variation_index: Option<VariationIndex>,

    /// A reason struct describing the main factor that influenced the flag evaluation value.
    pub reason: Reason,
}

impl<T> Detail<T> {
    /// Returns a detail with value and variation_index of None.
    ///
    /// If a flag does not have an appropriate fallback value, the [Detail::value] and
    /// [Detail::variation_index] must be None. In each case, the [Detail::reason] will be set to
    /// the reason provided to this method.
    pub fn empty(reason: Reason) -> Detail<T> {
        Detail {
            value: None,
            variation_index: None,
            reason,
        }
    }

    /// Returns a detail response using the provided default as the value and a variation_index
    /// of None.
    ///
    /// If the SDK variation methods detect some error condition, it will fallback to the user-provided
    /// default value. The provided error will be included as part of the [Detail::reason], and the
    /// [Detail::variation_index] will be set to None.
    pub fn err_default(error: Error, default: T) -> Detail<T> {
        Detail {
            value: Some(default),
            variation_index: None,
            reason: Reason::Error { error },
        }
    }

    /// Returns a detail response using the provided error as the [Detail::reason].
    pub fn err(error: Error) -> Detail<T> {
        Detail::empty(Reason::Error { error })
    }

    /// Returns a new instance of this detail with the provided function `f` applied to
    /// [Detail::value].
    pub fn map<U, F>(self, f: F) -> Detail<U>
    where
        F: FnOnce(T) -> U,
    {
        Detail {
            value: self.value.map(f),
            variation_index: self.variation_index,
            reason: self.reason,
        }
    }

    /// Sets the [Detail::reason] to the provided error if the current detail instance does not
    /// have a value set.
    pub fn should_have_value(mut self, e: Error) -> Detail<T> {
        if self.value.is_none() {
            self.reason = Reason::Error { error: e };
        }
        self
    }

    /// Returns a new instance of detail with the provided function `f` applied to
    /// [Detail::value] if it exists.
    ///
    /// [Detail::value] may or may not be set. If it is not set, this method will return a new
    /// [Detail] instance with the [Detail::reason] set to the provided [Error] `e`.
    ///
    /// If it is set, this method will apply the provided function `f` to the value. If the method
    /// `f` returns None, this method will return an error [Detail]. See [Detail::err]. Otherwise,
    /// a [Detail] instance will be returned with the result of the `f` application.
    pub fn try_map<U, F>(self, f: F, default: U, e: Error) -> Detail<U>
    where
        F: FnOnce(T) -> Option<U>,
    {
        if self.value.is_none() {
            return Detail {
                value: Some(default),
                variation_index: self.variation_index,
                reason: self.reason,
            };
        }
        match f(self.value.unwrap()) {
            Some(v) => Detail {
                value: Some(v),
                variation_index: self.variation_index,
                reason: self.reason,
            },
            None => Detail::err_default(e, default),
        }
    }

    /// Set the [Detail::value] to `default` if it does not exist.
    ///
    /// The SDK always wants to return an evaluation result. This method helps ensure that if a
    /// [Detail::value] is None, we can update it with the provided default.
    pub fn or(mut self, default: T) -> Detail<T> {
        if self.value.is_none() {
            self.value = Some(default);
            self.variation_index = None;
            // N.B. reason remains untouched: this is counterintuitive, but consistent with Go
        }
        self
    }

    /// Set the [Detail::value] to `default` if it does not exist.
    ///
    /// This method accomplishes the same thing as [Detail::or] but allows the default value to be
    /// provided through the result of a callback. This helps reduce computation where an
    /// evaluation default value might be costly to calculate and is likely infrequently used.
    pub fn or_else<F>(mut self, default: F) -> Detail<T>
    where
        F: Fn() -> T,
    {
        if self.value.is_none() {
            self.value = Some(default());
            self.variation_index = None;
        }
        self
    }
}

/// Reason describes the reason that a flag evaluation produced a particular value.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE", tag = "kind")]
pub enum Reason {
    /// Off indicates that the flag was off and therefore returned its configured off value.
    Off,
    /// TargetMatch indicates that context key was specifically targeted for this flag.
    TargetMatch,
    /// RuleMatch indicates that the context matched one of the flag's rules.
    #[serde(rename_all = "camelCase")]
    RuleMatch {
        /// Zero-based index of the [crate::FlagRule] that was matched.
        rule_index: usize,
        #[serde(skip_serializing_if = "String::is_empty")]
        /// The id property of the [crate::FlagRule::id] that was matched.
        rule_id: String,
        /// This optional boolean property is true if the variation was determined by a [crate::Rollout]
        /// whose kind was [crate::RolloutKind::Experiment] and if the selected [crate::WeightedVariation] did not have an
        /// untracked property of true. It is false otherwise.
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        in_experiment: bool,
    },
    /// PrerequisiteFailed indicates that the flag was considered off because it had at
    /// least one prerequisite flag that either was off or did not return the desired variation.
    #[serde(rename_all = "camelCase")]
    PrerequisiteFailed {
        /// The key of the prerequisite flag that failed.
        prerequisite_key: String,
    },
    /// Fallthrough indicates that the flag was on but the context did not match any targets
    /// or rules.
    #[serde(rename_all = "camelCase")]
    Fallthrough {
        /// This optional boolean property is true if the variation was determined by a [crate::Rollout]
        /// whose kind was [crate::RolloutKind::Experiment] and if the selected [crate::WeightedVariation] did not have an
        /// untracked property of true. It is false otherwise.
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        in_experiment: bool,
    },
    /// Error indicates that the flag could not be evaluated, e.g. because it does not
    /// exist or due to an unexpected error. In this case the result value will be the default value
    /// that the caller passed to the client.
    Error {
        /// An error representing the [Reason::Error].
        #[serde(rename = "errorKind")]
        error: Error,
    },
}

impl Reason {
    /// This method determines whether or not the provided [Reason] is considered to be part of an
    /// ongoing experiment.
    pub fn is_in_experiment(&self) -> bool {
        match self {
            Reason::RuleMatch { in_experiment, .. } => *in_experiment,
            Reason::Fallthrough { in_experiment } => *in_experiment,
            _ => false,
        }
    }
}

/// Error is returned via a [Reason::Error] when the client could not evaluate a flag, and
/// provides information about why the flag could not be evaluated.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Error {
    /// ClientNotReady indicates that the caller tried to evaluate a flag before the client
    /// had successfully initialized.
    ClientNotReady,
    /// FlagNotFound indicates that the caller provided a flag key that did not match any
    /// known flag.
    FlagNotFound,
    /// MalformedFlag indicates that there was an internal inconsistency in the flag data,
    /// e.g. a rule specified a nonexistent variation.
    MalformedFlag,
    /// WrongType indicates that the result value was not of the requested type, e.g. you
    /// called BoolVariationDetail but the value was an integer.
    WrongType,
    /// Exception indicates that an unexpected error stopped flag evaluation; check the
    /// log for details.
    Exception,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contexts::context::Kind;
    use crate::flag_value::FlagValue::{Bool, Str};
    use crate::test_common::{InMemoryPrerequisiteEventRecorder, TestStore};
    use crate::variation::VariationOrRollout;
    use crate::{AttributeValue, ContextBuilder, MultiContextBuilder};
    use spectral::prelude::*;
    use std::cell::RefCell;
    use test_case::test_case;

    #[test]
    fn test_eval_flag_basic() {
        let store = TestStore::new();
        let alice = ContextBuilder::new("alice").build().unwrap(); // not targeted
        let bob = ContextBuilder::new("bob").build().unwrap(); // targeted
        let mut flag = store.flag("flagWithTarget").unwrap();

        assert!(!flag.on);
        let detail = evaluate(&store, &flag, &alice, None);
        assert_that!(detail.value).contains_value(&Bool(false));
        assert_that!(detail.variation_index).contains_value(0);
        assert_that!(detail.reason).is_equal_to(&Reason::Off);

        assert_that!(evaluate(&store, &flag, &bob, None)).is_equal_to(&detail);

        // flip off variation
        flag.off_variation = Some(1);
        let detail = evaluate(&store, &flag, &alice, None);
        assert_that!(detail.value).contains_value(&Bool(true));
        assert_that!(detail.variation_index).contains_value(1);

        // off variation unspecified
        flag.off_variation = None;
        let detail = evaluate(&store, &flag, &alice, None);
        assert_that!(detail.value).is_none();
        assert_that!(detail.variation_index).is_none();
        assert_that!(detail.reason).is_equal_to(&Reason::Off);

        // flip targeting on
        flag.on = true;
        let detail = evaluate(&store, &flag, &alice, None);
        assert_that!(detail.value).contains_value(&Bool(true));
        assert_that!(detail.variation_index).contains_value(1);
        assert_that!(detail.reason).is_equal_to(&Reason::Fallthrough {
            in_experiment: false,
        });

        let detail = evaluate(&store, &flag, &bob, None);
        assert_that!(detail.value).contains_value(&Bool(false));
        assert_that!(detail.variation_index).contains_value(0);
        assert_that!(detail.reason).is_equal_to(&Reason::TargetMatch);

        // flip default variation
        flag.fallthrough = VariationOrRollout::Variation { variation: 0 };
        let detail = evaluate(&store, &flag, &alice, None);
        assert_that!(detail.value).contains_value(&Bool(false));
        assert_that!(detail.variation_index).contains_value(0);

        // bob's reason should still be TargetMatch even though his value is now the default
        let detail = evaluate(&store, &flag, &bob, None);
        assert_that!(detail.value).contains_value(&Bool(false));
        assert_that!(detail.variation_index).contains_value(0);
        assert_that!(detail.reason).is_equal_to(&Reason::TargetMatch);
    }

    #[test]
    fn test_eval_with_matches_op_groups() {
        let store = TestStore::new();
        let alice = ContextBuilder::new("alice").build().unwrap(); // not targeted
        let mut bob_builder = ContextBuilder::new("bob");
        bob_builder.try_set_value(
            "groups",
            AttributeValue::Array(vec![AttributeValue::String("my-group".into())]),
        );
        let bob = bob_builder.build().unwrap(); // targeted
        let flag = store.flag("flagWithMatchesOpOnGroups").unwrap();

        let detail = evaluate(&store, &flag, &alice, None);
        assert_that!(detail.value).contains_value(&Bool(true));
        assert_that!(detail.reason).is_equal_to(&Reason::Fallthrough {
            in_experiment: false,
        });

        let detail = evaluate(&store, &flag, &bob, None);
        assert_that!(detail.value).contains_value(&Bool(false));
        assert_that!(detail.variation_index).contains_value(0);
        assert_that!(detail.reason).is_equal_to(&Reason::RuleMatch {
            rule_index: 0,
            rule_id: "6a7755ac-e47a-40ea-9579-a09dd5f061bd".into(),
            in_experiment: false,
        });
    }

    #[test_case("flagWithMatchesOpOnKinds")]
    // Note that the following two cases should not be seen in real data, since
    // rules that are meant to have special kind matching logic will omit contextKind altogether.
    #[test_case("flagWithMatchesOpOnKindsAttributeReference")]
    #[test_case("flagWithMatchesOpOnKindsPlainAttributeReference")]
    // This test checks the special behavior of specifying an attribute reference that
    // resolves to 'kind' in a clause. In this case, the clause is meant to apply to the
    // context's kind(s), for example detecting if a (multi) context contains a certain kind.
    fn test_eval_with_matches_op_kinds(flag_key: &str) {
        let store = TestStore::new();

        let alice = ContextBuilder::new("alice").build().unwrap(); // targeted

        let mut bob_builder = ContextBuilder::new("bob");
        let bob = bob_builder.kind("company").build().unwrap(); // not targeted

        let flag = store.flag(flag_key).unwrap();

        let detail = evaluate(&store, &flag, &alice, None);

        assert_that!(detail.value).contains_value(&Bool(false));
        assert_that!(detail.variation_index).contains_value(0);
        assert_that!(detail.reason).is_equal_to(&Reason::RuleMatch {
            rule_index: 0,
            rule_id: "6a7755ac-e47a-40ea-9579-a09dd5f061bd".into(),
            in_experiment: false,
        });

        let detail = evaluate(&store, &flag, &bob, None);
        assert_that!(detail.value).contains_value(&Bool(true));
        assert_that!(detail.reason).is_equal_to(&Reason::Fallthrough {
            in_experiment: false,
        });

        bob_builder.kind("org");
        let new_bob = bob_builder.build().unwrap(); // re-targeted
        let detail = evaluate(&store, &flag, &new_bob, None);
        assert_that!(detail.value).contains_value(&Bool(false));
        assert_that!(detail.variation_index).contains_value(0);
        assert_that!(detail.reason).is_equal_to(&Reason::RuleMatch {
            rule_index: 0,
            rule_id: "6a7755ac-e47a-40ea-9579-a09dd5f061bd".into(),
            in_experiment: false,
        });
    }

    #[test]
    fn test_prerequisite_events_are_captured() {
        let recorder = InMemoryPrerequisiteEventRecorder {
            events: RefCell::new(Vec::new()),
        };
        let store = TestStore::new();
        let alice = ContextBuilder::new("alice").build().unwrap();
        let flag = store.flag("flagWithNestedPrereq").unwrap();

        let _ = evaluate(&store, &flag, &alice, Some(&recorder));
        assert_that!(*recorder.events.borrow()).has_length(2);

        let event = &recorder.events.borrow()[0];
        assert_eq!("flagWithSatisfiedPrereq", event.target_flag_key);
        assert_eq!("prereq", event.prerequisite_flag.key);

        let event = &recorder.events.borrow()[1];
        assert_eq!("flagWithNestedPrereq", event.target_flag_key);
        assert_eq!("flagWithSatisfiedPrereq", event.prerequisite_flag.key);
    }

    #[test]
    fn test_eval_flag_rules() {
        let store = TestStore::new();
        let alice = ContextBuilder::new("alice").build().unwrap();
        let bob = ContextBuilder::new("bob")
            .set_value("team", "Avengers".into())
            .build()
            .unwrap();

        let mut flag = store.flag("flagWithInRule").unwrap();

        assert!(!flag.on);
        for context in &[&alice, &bob] {
            let detail = evaluate(&store, &flag, context, None);
            assert_that!(detail.value).contains_value(&Bool(false));
            assert_that!(detail.variation_index).contains_value(0);
            assert_that!(detail.reason).is_equal_to(&Reason::Off);
        }

        // flip targeting on
        flag.on = true;
        let detail = evaluate(&store, &flag, &alice, None);
        assert_that!(detail.value).contains_value(&Bool(true));
        assert_that!(detail.variation_index).contains_value(1);
        assert_that!(detail.reason).is_equal_to(&Reason::Fallthrough {
            in_experiment: false,
        });

        let detail = evaluate(&store, &flag, &bob, None);
        assert_that!(detail.value).contains_value(&Bool(false));
        assert_that!(detail.variation_index).contains_value(0);
        assert_that!(detail.reason).is_equal_to(&Reason::RuleMatch {
            rule_id: "in-rule".to_string(),
            rule_index: 0,
            in_experiment: false,
        });
    }

    #[test]
    fn test_eval_flag_unsatisfied_prereq() {
        let store = TestStore::new();
        let flag = store.flag("flagWithMissingPrereq").unwrap();
        assert!(flag.on);

        let alice = ContextBuilder::new("alice").build().unwrap();
        let bob = ContextBuilder::new("bob").build().unwrap();

        for user in &[&alice, &bob] {
            let detail = evaluate(&store, &flag, user, None);
            assert_that!(detail.value).contains_value(&Bool(false));
            assert_that!(detail.reason).is_equal_to(&Reason::PrerequisiteFailed {
                prerequisite_key: "badPrereq".to_string(),
            });
        }
    }

    #[test]
    fn test_eval_flag_off_prereq() {
        let store = TestStore::new();
        let flag = store.flag("flagWithOffPrereq").unwrap();
        assert!(flag.on);

        let alice = ContextBuilder::new("alice").build().unwrap();

        let detail = evaluate(&store, &flag, &alice, None);
        assert_that!(detail.value).contains_value(&Bool(false));
        assert_that!(detail.reason).is_equal_to(&Reason::PrerequisiteFailed {
            prerequisite_key: "offPrereq".to_string(),
        });
    }

    #[test]
    fn test_eval_flag_satisfied_prereq() {
        let mut store = TestStore::new();
        let flag = store.flag("flagWithSatisfiedPrereq").unwrap();

        let alice = ContextBuilder::new("alice").build().unwrap();
        let bob = ContextBuilder::new("bob").build().unwrap();

        let detail = evaluate(&store, &flag, &alice, None);
        asserting!("alice should pass prereq and see fallthrough")
            .that(&detail.value)
            .contains_value(&Bool(true));
        let detail = evaluate(&store, &flag, &bob, None);
        asserting!("bob should see prereq failed due to target")
            .that(&detail.value)
            .contains_value(&Bool(false));
        assert_that!(detail.reason).is_equal_to(Reason::PrerequisiteFailed {
            prerequisite_key: "prereq".to_string(),
        });

        // prerequisite off
        store.update_flag("prereq", |flag| (flag.on = false));
        for user in &[&alice, &bob] {
            let detail = evaluate(&store, &flag, user, None);
            assert_that!(detail.value).contains_value(&Bool(false));
            assert_that!(detail.reason).is_equal_to(&Reason::PrerequisiteFailed {
                prerequisite_key: "prereq".to_string(),
            });
        }
    }

    #[test]
    fn test_flag_targets_different_context() {
        let store = TestStore::new();
        let alice_user_context = ContextBuilder::new("alice").build().unwrap();
        let bob_user_context = ContextBuilder::new("bob").build().unwrap();
        let org_context = ContextBuilder::new("LaunchDarkly")
            .kind("org")
            .build()
            .unwrap();
        let flag = store.flag("flagWithContextTarget").unwrap();

        let detail = evaluate(&store, &flag, &org_context, None);
        assert_that!(detail.value).contains_value(&Bool(true));
        assert_that!(detail.variation_index).contains_value(1);
        assert_that!(detail.reason).is_equal_to(&Reason::TargetMatch);

        let detail = evaluate(&store, &flag, &alice_user_context, None);
        assert_that!(detail.value).contains_value(&Bool(false));
        assert_that!(detail.variation_index).contains_value(0);
        assert_that!(detail.reason).is_equal_to(&Reason::Fallthrough {
            in_experiment: false,
        });

        let detail = evaluate(&store, &flag, &bob_user_context, None);
        assert_that!(detail.value).contains_value(&Bool(true));
        assert_that!(detail.variation_index).contains_value(1);
        assert_that!(detail.reason).is_equal_to(&Reason::TargetMatch);
    }

    #[test]
    fn test_eval_flag_segments() {
        let store = TestStore::new();
        let flag = store.flag("flagWithSegmentMatchRule").unwrap();

        let alice = ContextBuilder::new("alice").build().unwrap();
        let bob = ContextBuilder::new("bob").build().unwrap();

        let detail = evaluate(&store, &flag, &alice, None);
        asserting!("alice is in segment, should see false with RuleMatch")
            .that(&detail.value)
            .contains_value(&Bool(false));
        assert_that!(detail.reason).is_equal_to(Reason::RuleMatch {
            rule_id: "match-rule".to_string(),
            rule_index: 0,
            in_experiment: false,
        });
        let detail = evaluate(&store, &flag, &bob, None);
        asserting!("bob is not in segment and should see fallthrough")
            .that(&detail.value)
            .contains_value(&Bool(true));
        assert_that!(detail.reason).is_equal_to(Reason::Fallthrough {
            in_experiment: false,
        });
    }

    #[test]
    fn test_flag_has_prereq_which_duplicates_segment_rule() {
        let store = TestStore::new();
        let flag = store
            .flag("flagWithPrereqWhichDuplicatesSegmentRuleCheck")
            .unwrap();

        let alice = ContextBuilder::new("alice").build().unwrap();

        let mut evaluation_stack = EvaluationStack::default();
        let detail = evaluate_internal(&store, &flag, &alice, None, &mut evaluation_stack);
        asserting!("alice is in segment, should see false with RuleMatch")
            .that(&detail.value)
            .contains_value(&Bool(false));
        assert_that!(detail.reason).is_equal_to(Reason::RuleMatch {
            rule_id: "match-rule".to_string(),
            rule_index: 0,
            in_experiment: false,
        });
        assert!(evaluation_stack.prerequisite_flag_chain.is_empty());
        assert!(evaluation_stack.segment_chain.is_empty());
    }

    // Flag A
    //   Flag B
    //     Flag A
    #[test]
    fn test_simple_prereq_cycle() {
        let flag_json = r#"{
            "flagA": {
                "key": "flagA",
                "targets": [],
                "rules": [],
                "salt": "salty",
                "prerequisites": [{
                    "key": "flagB",
                    "variation": 0
                }],
                "on": true,
                "fallthrough": {"variation": 0},
                "offVariation": 1,
                "variations": [true, false]
            },
            "flagB": {
                "key": "flagB",
                "targets": [],
                "rules": [],
                "salt": "salty",
                "prerequisites": [{
                    "key": "flagA",
                    "variation": 0
                }],
                "on": true,
                "fallthrough": {"variation": 0},
                "offVariation": 1,
                "variations": [true, false]
            }
        }"#;
        let store = TestStore::new_from_json_str(flag_json, "{}");
        let flag = store.flag("flagA").unwrap();

        let alice = ContextBuilder::new("alice").build().unwrap();

        let detail = evaluate(&store, &flag, &alice, None);
        assert_that!(detail.value).is_none();
        assert_that!(detail.reason).is_equal_to(Reason::Error {
            error: Error::MalformedFlag,
        });
    }

    // Flag A
    //   Flag B
    //   Flag C
    //     Flag B
    #[test]
    fn test_eval_flag_with_first_prereq_as_prereq_of_second_prereq() {
        let store = TestStore::new();
        let flag = store
            .flag("flagWithFirstPrereqAsPrereqToSecondPrereq")
            .unwrap();

        let alice = ContextBuilder::new("alice").build().unwrap();
        let bob = ContextBuilder::new("bob").build().unwrap();

        let mut evaluation_stack = EvaluationStack::default();
        let detail = evaluate_internal(&store, &flag, &alice, None, &mut evaluation_stack);
        asserting!("alice should pass prereq and see fallthrough")
            .that(&detail.value)
            .contains_value(&Bool(true));
        assert!(evaluation_stack.prerequisite_flag_chain.is_empty());
        assert!(evaluation_stack.segment_chain.is_empty());

        let detail = evaluate(&store, &flag, &bob, None);
        asserting!("bob should see prereq failed due to target")
            .that(&detail.value)
            .contains_value(&Bool(false));
        assert_that!(detail.reason).is_equal_to(Reason::PrerequisiteFailed {
            prerequisite_key: "prereq".to_string(),
        });
    }

    // Flag A
    //   Flag B
    //     Flag C
    // Flag C
    //   Flag A
    #[test]
    fn test_prereq_cycle_across_three_flags() {
        let flag_json = r#"{
            "flagA": {
                "key": "flagA",
                "targets": [],
                "rules": [],
                "salt": "salty",
                "prerequisites": [{
                    "key": "flagB",
                    "variation": 0
                }],
                "on": true,
                "fallthrough": {"variation": 0},
                "offVariation": 1,
                "variations": [true, false]
            },
            "flagB": {
                "key": "flagB",
                "targets": [],
                "rules": [],
                "salt": "salty",
                "prerequisites": [{
                    "key": "flagC",
                    "variation": 0
                }],
                "on": true,
                "fallthrough": {"variation": 0},
                "offVariation": 1,
                "variations": [true, false]
            },
            "flagC": {
                "key": "flagC",
                "targets": [],
                "rules": [],
                "salt": "salty",
                "prerequisites": [{
                    "key": "flagA",
                    "variation": 0
                }],
                "on": true,
                "fallthrough": {"variation": 0},
                "offVariation": 1,
                "variations": [true, false]
            }
        }"#;
        let store = TestStore::new_from_json_str(flag_json, "{}");
        let flag = store.flag("flagA").unwrap();

        let alice = ContextBuilder::new("alice").build().unwrap();

        let detail = evaluate(&store, &flag, &alice, None);
        assert_that!(detail.value).is_none();
        assert_that!(detail.reason).is_equal_to(Reason::Error {
            error: Error::MalformedFlag,
        });
    }

    // Flag A Segment A
    // Flag B Segment A
    #[test]
    fn test_flag_and_prereq_share_segment_check() {
        let flag_json = r#"{
            "flagA": {
                "key": "flagA",
                "targets": [],
                "rules": [{
                    "variation": 0,
                    "id": "rule-1",
                    "clauses": [
                        {
                            "contextKind": "user",
                            "attribute": "key",
                            "negate": false,
                            "op": "segmentMatch",
                            "values": ["segmentA"]
                        }
                    ],
                    "trackEvents": true
                }],
                "salt": "salty",
                "prerequisites": [{
                    "key": "flagB",
                    "variation": 0
                }],
                "on": true,
                "fallthrough": {"variation": 0},
                "offVariation": 1,
                "variations": [true, false]
            },
            "flagB": {
                "key": "flagB",
                "targets": [],
                "rules": [{
                    "variation": 0,
                    "id": "rule-2",
                    "clauses": [
                        {
                            "contextKind": "user",
                            "attribute": "key",
                            "negate": false,
                            "op": "segmentMatch",
                            "values": ["segmentA"]
                        }
                    ],
                    "trackEvents": true
                }],
                "salt": "salty",
                "prerequisites": [],
                "on": true,
                "fallthrough": {"variation": 0},
                "offVariation": 1,
                "variations": [true, false]
            }
        }"#;
        let segment_json = r#"{
            "segmentA": {
                "key": "segmentA",
                "included": ["alice"],
                "includedContexts": [{
                    "values": [],
                    "contextKind": "user"
                }],
                "excluded": [],
                "rules": [],
                "salt": "salty",
                "version": 1
            }
        }"#;
        let store = TestStore::new_from_json_str(flag_json, segment_json);
        let flag = store.flag("flagA").unwrap();

        let alice = ContextBuilder::new("alice").build().unwrap();

        let mut evaluation_stack = EvaluationStack::default();
        let detail = evaluate_internal(&store, &flag, &alice, None, &mut evaluation_stack);
        assert_that!(detail.value).contains_value(&Bool(true));
        assert_that!(detail.reason).is_equal_to(Reason::RuleMatch {
            rule_index: 0,
            rule_id: "rule-1".into(),
            in_experiment: false,
        });
        assert!(evaluation_stack.prerequisite_flag_chain.is_empty());
        assert!(evaluation_stack.segment_chain.is_empty());
    }

    // Flag A
    //   Flag B Segment A
    //   Flag C Segment A
    #[test]
    fn test_prereqs_can_share_segment_check() {
        let flag_json = r#"{
            "flagA": {
                "key": "flagA",
                "targets": [],
                "rules": [],
                "salt": "salty",
                "prerequisites": [
                    {
                        "key": "flagB",
                        "variation": 0
                    },
                    {
                        "key": "flagC",
                        "variation": 0
                    }
                ],
                "on": true,
                "fallthrough": {"variation": 0},
                "offVariation": 1,
                "variations": [true, false]
            },
            "flagB": {
                "key": "flagB",
                "targets": [],
                "rules": [{
                    "variation": 0,
                    "id": "rule-2",
                    "clauses": [
                        {
                            "contextKind": "user",
                            "attribute": "key",
                            "negate": false,
                            "op": "segmentMatch",
                            "values": ["segmentA"]
                        }
                    ],
                    "trackEvents": true
                }],
                "salt": "salty",
                "prerequisites": [],
                "on": true,
                "fallthrough": {"variation": 0},
                "offVariation": 1,
                "variations": [true, false]
            },
            "flagC": {
                "key": "flagC",
                "targets": [],
                "rules": [{
                    "variation": 0,
                    "id": "rule-2",
                    "clauses": [
                        {
                            "contextKind": "user",
                            "attribute": "key",
                            "negate": false,
                            "op": "segmentMatch",
                            "values": ["segmentA"]
                        }
                    ],
                    "trackEvents": true
                }],
                "salt": "salty",
                "prerequisites": [],
                "on": true,
                "fallthrough": {"variation": 0},
                "offVariation": 1,
                "variations": [true, false]
            }
        }"#;
        let segment_json = r#"{
            "segmentA": {
                "key": "segmentA",
                "included": ["alice"],
                "includedContexts": [{
                    "values": [],
                    "contextKind": "user"
                }],
                "excluded": [],
                "rules": [],
                "salt": "salty",
                "version": 1
            }
        }"#;
        let store = TestStore::new_from_json_str(flag_json, segment_json);
        let flag = store.flag("flagA").unwrap();

        let alice = ContextBuilder::new("alice").build().unwrap();

        let mut evaluation_stack = EvaluationStack::default();
        let detail = evaluate_internal(&store, &flag, &alice, None, &mut evaluation_stack);
        assert_that!(detail.value).contains_value(&Bool(true));
        assert_that!(detail.reason).is_equal_to(Reason::Fallthrough {
            in_experiment: false,
        });
        assert!(evaluation_stack.prerequisite_flag_chain.is_empty());
        assert!(evaluation_stack.segment_chain.is_empty());
    }

    // Flag A Segment A
    // Segment A
    //  Segment B
    //   Segment A
    #[test]
    fn test_segment_has_basic_recursive_condition() {
        let flag_json = r#"{
            "flagA": {
                "key": "flagA",
                "targets": [],
                "rules": [{
                    "variation": 0,
                    "id": "rule-1",
                    "clauses": [
                        {
                            "contextKind": "user",
                            "attribute": "key",
                            "negate": false,
                            "op": "segmentMatch",
                            "values": ["segmentA"]
                        }
                    ],
                    "trackEvents": true
                }],
                "salt": "salty",
                "prerequisites": [],
                "on": true,
                "fallthrough": {"variation": 0},
                "offVariation": 1,
                "variations": [true, false]
            }
        }"#;
        let segment_json = r#"{
            "segmentA": {
                "key": "segmentA",
                "included": [],
                "includedContexts": [],
                "excluded": [],
                "rules": [{
                    "id": "rule-1",
                    "clauses": [{
                        "attribute": "key",
                        "negate": false,
                        "op": "segmentMatch",
                        "values": ["segmentB"],
                        "contextKind": "user"
                    }]
                }],
                "salt": "salty",
                "version": 1
            },
            "segmentB": {
                "key": "segmentB",
                "included": [],
                "includedContexts": [],
                "excluded": [],
                "rules": [{
                    "id": "rule-1",
                    "clauses": [{
                        "attribute": "key",
                        "negate": false,
                        "op": "segmentMatch",
                        "values": ["segmentA"],
                        "contextKind": "user"
                    }]
                }],
                "salt": "salty",
                "version": 1
            }
        }"#;
        let store = TestStore::new_from_json_str(flag_json, segment_json);
        let flag = store.flag("flagA").unwrap();

        let alice = ContextBuilder::new("alice").build().unwrap();

        let detail = evaluate(&store, &flag, &alice, None);
        assert_that!(detail.value).is_none();
        assert_that!(detail.reason).is_equal_to(Reason::Error {
            error: Error::MalformedFlag,
        });
    }

    // Flag A Segment A
    // Segment A
    //  Segment A
    #[test]
    fn test_segment_depends_on_self() {
        let flag_json = r#"{
            "flagA": {
                "key": "flagA",
                "targets": [],
                "rules": [{
                    "variation": 0,
                    "id": "rule-1",
                    "clauses": [
                        {
                            "contextKind": "user",
                            "attribute": "key",
                            "negate": false,
                            "op": "segmentMatch",
                            "values": ["segmentA"]
                        }
                    ],
                    "trackEvents": true
                }],
                "salt": "salty",
                "prerequisites": [],
                "on": true,
                "fallthrough": {"variation": 0},
                "offVariation": 1,
                "variations": [true, false]
            }
        }"#;
        let segment_json = r#"{
            "segmentA": {
                "key": "segmentA",
                "included": [],
                "includedContexts": [],
                "excluded": [],
                "rules": [{
                    "id": "rule-1",
                    "clauses": [{
                        "attribute": "key",
                        "negate": false,
                        "op": "segmentMatch",
                        "values": ["segmentA"],
                        "contextKind": "user"
                    }]
                }],
                "salt": "salty",
                "version": 1
            }
        }"#;
        let store = TestStore::new_from_json_str(flag_json, segment_json);
        let flag = store.flag("flagA").unwrap();

        let alice = ContextBuilder::new("alice").build().unwrap();

        let detail = evaluate(&store, &flag, &alice, None);
        assert_that!(detail.value).is_none();
        assert_that!(detail.reason).is_equal_to(Reason::Error {
            error: Error::MalformedFlag,
        });
    }

    // Flag A Segment A
    //   Flag B Segment B
    // Segment A
    //   Segment B
    //     Segment C
    #[test]
    fn test_flag_has_segment_check_and_prereq_also_has_subset_of_segment_checks() {
        let flag_json = r#"{
            "flagA": {
                "key": "flagA",
                "targets": [],
                "rules": [{
                    "variation": 0,
                    "id": "rule-1",
                    "clauses": [
                        {
                            "contextKind": "user",
                            "attribute": "key",
                            "negate": false,
                            "op": "segmentMatch",
                            "values": ["segmentA"]
                        }
                    ],
                    "trackEvents": true
                }],
                "salt": "salty",
                "prerequisites": [{
                    "key": "flagB",
                    "variation": 0
                }],
                "on": true,
                "fallthrough": {"variation": 0},
                "offVariation": 1,
                "variations": [true, false]
            },
            "flagB": {
                "key": "flagB",
                "targets": [],
                "rules": [{
                    "variation": 0,
                    "id": "rule-2",
                    "clauses": [
                        {
                            "contextKind": "user",
                            "attribute": "key",
                            "negate": false,
                            "op": "segmentMatch",
                            "values": ["segmentB"]
                        }
                    ],
                    "trackEvents": true
                }],
                "salt": "salty",
                "prerequisites": [],
                "on": true,
                "fallthrough": {"variation": 0},
                "offVariation": 1,
                "variations": [true, false]
            }
        }"#;
        let segment_json = r#"{
            "segmentA": {
                "key": "segmentA",
                "included": [],
                "includedContexts": [],
                "excluded": [],
                "rules": [{
                    "id": "rule-1",
                    "clauses": [{
                        "attribute": "key",
                        "negate": false,
                        "op": "segmentMatch",
                        "values": ["segmentB"],
                        "contextKind": "user"
                    }]
                }],
                "salt": "salty",
                "version": 1
            },
            "segmentB": {
                "key": "segmentB",
                "included": [],
                "includedContexts": [],
                "excluded": [],
                "rules": [{
                    "id": "rule-1",
                    "clauses": [{
                        "attribute": "key",
                        "negate": false,
                        "op": "segmentMatch",
                        "values": ["segmentC"],
                        "contextKind": "user"
                    }]
                }],
                "salt": "salty",
                "version": 1
            },
            "segmentC": {
                "key": "segmentC",
                "included": ["alice"],
                "includedContexts": [],
                "excluded": ["bob"],
                "rules": [],
                "salt": "salty",
                "version": 1
            }
        }"#;
        let store = TestStore::new_from_json_str(flag_json, segment_json);
        let flag = store.flag("flagA").unwrap();

        let alice = ContextBuilder::new("alice").build().unwrap();
        let bob = ContextBuilder::new("bob").build().unwrap();

        let mut evaluation_stack = EvaluationStack::default();
        let detail = evaluate_internal(&store, &flag, &alice, None, &mut evaluation_stack);
        assert_that!(detail.value).contains_value(&Bool(true));
        assert_that!(detail.reason).is_equal_to(&Reason::RuleMatch {
            rule_index: 0,
            rule_id: "rule-1".to_string(),
            in_experiment: false,
        });
        assert!(evaluation_stack.prerequisite_flag_chain.is_empty());
        assert!(evaluation_stack.segment_chain.is_empty());

        let mut evaluation_stack = EvaluationStack::default();
        let detail = evaluate_internal(&store, &flag, &bob, None, &mut evaluation_stack);
        assert_that!(detail.value).contains_value(&Bool(true));
        assert_that!(detail.reason).is_equal_to(&Reason::Fallthrough {
            in_experiment: false,
        });
        assert!(evaluation_stack.prerequisite_flag_chain.is_empty());
        assert!(evaluation_stack.segment_chain.is_empty());
    }

    #[test]
    fn test_rollout_flag() {
        let store = TestStore::new();
        let flag = store.flag("flagWithRolloutBucketBy").unwrap();

        let alice = ContextBuilder::new("anonymous")
            .set_value("platform", "aem".into())
            .set_value("ld_quid", "d4ad12cb-392b-4fce-b214-843ad625d6f8".into())
            .build()
            .unwrap();

        let detail = evaluate(&store, &flag, &alice, None);
        assert_that!(detail.value).contains_value(&Str("rollout1".to_string()));
    }

    #[test]
    fn test_experiment_flag() {
        let store = TestStore::new();
        let flag = store.flag("flagWithExperiment").unwrap();

        let user_a = ContextBuilder::new("userKeyA").build().unwrap();
        let detail = evaluate(&store, &flag, &user_a, None);
        assert_that!(detail.value).contains_value(&Bool(false));
        assert!(detail.reason.is_in_experiment());

        let user_b = ContextBuilder::new("userKeyB").build().unwrap();
        let detail = evaluate(&store, &flag, &user_b, None);
        assert_that!(detail.value).contains_value(&Bool(true));
        assert!(detail.reason.is_in_experiment());

        let user_c = ContextBuilder::new("userKeyC").build().unwrap();
        let detail = evaluate(&store, &flag, &user_c, None);
        assert_that!(detail.value).contains_value(&Bool(false));
        assert!(!detail.reason.is_in_experiment());
    }

    #[test]
    fn test_experiment_flag_targeting_missing_context() {
        let store = TestStore::new();
        let flag = store.flag("flagWithExperimentTargetingContext").unwrap();

        let user_a = ContextBuilder::new("userKeyA").build().unwrap();
        let detail = evaluate(&store, &flag, &user_a, None);
        assert_that!(detail.value).contains_value(&Bool(false));
        assert_that!(detail.reason).is_equal_to(Reason::Fallthrough {
            in_experiment: false,
        })
    }

    #[test]
    fn test_malformed_rule() {
        let store = TestStore::new();
        let mut flag = store.flag("flagWithMalformedRule").unwrap();

        let user_a = ContextBuilder::new("no").build().unwrap();
        let user_b = ContextBuilder::new("yes").build().unwrap();

        let detail = evaluate(&store, &flag, &user_a, None);
        assert_that!(detail.value).contains_value(&Bool(false));
        assert_that!(detail.reason).is_equal_to(Reason::Off);

        let detail = evaluate(&store, &flag, &user_b, None);
        assert_that!(detail.value).contains_value(&Bool(false));
        assert_that!(detail.reason).is_equal_to(Reason::Off);

        flag.on = true;

        let detail = evaluate(&store, &flag, &user_a, None);
        assert_that!(detail.value).contains_value(&Bool(true));
        assert_that!(detail.reason).is_equal_to(Reason::Fallthrough {
            in_experiment: false,
        });

        let detail = evaluate(&store, &flag, &user_b, None);
        assert_that!(detail.value).is_none();
        assert_that!(detail.reason).is_equal_to(Reason::Error {
            error: Error::MalformedFlag,
        });
    }

    #[test]
    fn reason_serialization() {
        struct Case<'a> {
            reason: Reason,
            json: &'a str,
        }

        let cases = vec![
            Case {
                reason: Reason::Off,
                json: r#"{"kind":"OFF"}"#,
            },
            Case {
                reason: Reason::Fallthrough {
                    in_experiment: false,
                },
                json: r#"{"kind":"FALLTHROUGH"}"#,
            },
            Case {
                reason: Reason::Fallthrough {
                    in_experiment: true,
                },
                json: r#"{"kind":"FALLTHROUGH","inExperiment":true}"#,
            },
            Case {
                reason: Reason::TargetMatch {},
                json: r#"{"kind":"TARGET_MATCH"}"#,
            },
            Case {
                reason: Reason::RuleMatch {
                    rule_index: 1,
                    rule_id: "x".into(),
                    in_experiment: false,
                },
                json: r#"{"kind":"RULE_MATCH","ruleIndex":1,"ruleId":"x"}"#,
            },
            Case {
                reason: Reason::RuleMatch {
                    rule_index: 1,
                    rule_id: "x".into(),
                    in_experiment: true,
                },
                json: r#"{"kind":"RULE_MATCH","ruleIndex":1,"ruleId":"x","inExperiment":true}"#,
            },
            Case {
                reason: Reason::PrerequisiteFailed {
                    prerequisite_key: "x".into(),
                },
                json: r#"{"kind":"PREREQUISITE_FAILED","prerequisiteKey":"x"}"#,
            },
            Case {
                reason: Reason::Error {
                    error: Error::WrongType,
                },
                json: r#"{"kind":"ERROR","errorKind":"WRONG_TYPE"}"#,
            },
        ];

        for Case {
            reason,
            json: expected_json,
        } in cases
        {
            let json = serde_json::to_string(&reason).unwrap();
            assert_eq!(
                expected_json, json,
                "unexpected serialization: {:?}",
                reason
            );
        }
    }

    #[test]
    fn get_applicable_context_by_kind_returns_correct_context() {
        let org_kind = Kind::from("org");
        let user_kind = Kind::from("user");
        let company_kind = Kind::from("company");

        let user_context = ContextBuilder::new("user").build().unwrap();
        let org_context = ContextBuilder::new("org").kind("org").build().unwrap();

        assert!(user_context.as_kind(&user_kind).is_some());
        assert!(user_context.as_kind(&org_kind).is_none());
        assert!(org_context.as_kind(&org_kind).is_some());

        let multi_context = MultiContextBuilder::new()
            .add_context(user_context)
            .add_context(org_context)
            .build()
            .unwrap();

        assert_eq!(
            &user_kind,
            multi_context.as_kind(&user_kind).unwrap().kind()
        );
        assert_eq!(&org_kind, multi_context.as_kind(&org_kind).unwrap().kind());
        assert!(multi_context.as_kind(&company_kind).is_none());
    }

    #[test]
    fn can_create_error_detail() {
        let detail = Detail::err_default(Error::MalformedFlag, true.into());

        assert_eq!(Some(AttributeValue::Bool(true)), detail.value);
        assert!(detail.variation_index.is_none());
        assert_that!(detail.reason).is_equal_to(Reason::Error {
            error: Error::MalformedFlag,
        });
    }

    #[test]
    fn can_force_error_if_value_is_none() {
        let detail: Detail<AttributeValue> = Detail {
            value: None,
            variation_index: None,
            reason: Reason::Off,
        };

        let detail = detail.should_have_value(Error::MalformedFlag);

        assert!(detail.value.is_none());
        assert!(detail.variation_index.is_none());
        assert_that!(detail.reason).is_equal_to(Reason::Error {
            error: Error::MalformedFlag,
        });
    }

    #[test]
    fn can_map_detail_with_default_and_error() {
        let detail: Detail<AttributeValue> = Detail {
            value: None,
            variation_index: None,
            reason: Reason::Off,
        };

        let mapped = detail.try_map(Some, false.into(), Error::MalformedFlag);
        assert_eq!(Some(AttributeValue::Bool(false)), mapped.value);
        assert!(mapped.variation_index.is_none());
        assert_that!(mapped.reason).is_equal_to(Reason::Off);

        let detail: Detail<AttributeValue> = Detail {
            value: Some(true.into()),
            variation_index: Some(1),
            reason: Reason::Off,
        };

        let mapped = detail.try_map(|_| Some(false.into()), false.into(), Error::MalformedFlag);
        assert_eq!(Some(AttributeValue::Bool(false)), mapped.value);
        assert_eq!(Some(1), mapped.variation_index);
        assert_that!(mapped.reason).is_equal_to(Reason::Off);

        let detail: Detail<AttributeValue> = Detail {
            value: Some(true.into()),
            variation_index: Some(1),
            reason: Reason::Off,
        };

        let mapped = detail.try_map(|_| None, false.into(), Error::MalformedFlag);
        assert_eq!(Some(AttributeValue::Bool(false)), mapped.value);
        assert!(mapped.variation_index.is_none());
        assert_that!(mapped.reason).is_equal_to(Reason::Error {
            error: Error::MalformedFlag,
        });
    }

    #[test]
    fn can_set_value_to_default_if_does_not_exist() {
        let detail: Detail<AttributeValue> = Detail {
            value: Some(true.into()),
            variation_index: Some(1),
            reason: Reason::Off,
        };

        let or_detail = detail.or(false.into());
        assert_eq!(Some(AttributeValue::Bool(true)), or_detail.value);
        assert_eq!(Some(1), or_detail.variation_index);
        assert_that!(or_detail.reason).is_equal_to(Reason::Off);

        let detail: Detail<AttributeValue> = Detail {
            value: None,
            variation_index: Some(1),
            reason: Reason::Off,
        };

        let or_detail = detail.or(false.into());
        assert_eq!(Some(AttributeValue::Bool(false)), or_detail.value);
        assert!(or_detail.variation_index.is_none());
        assert_that!(or_detail.reason).is_equal_to(Reason::Off);
    }

    #[test]
    fn can_set_value_to_default_if_does_not_exist_through_callback() {
        let detail: Detail<AttributeValue> = Detail {
            value: Some(true.into()),
            variation_index: Some(1),
            reason: Reason::Off,
        };

        let or_detail = detail.or_else(|| false.into());
        assert_eq!(Some(AttributeValue::Bool(true)), or_detail.value);
        assert_eq!(Some(1), or_detail.variation_index);
        assert_that!(or_detail.reason).is_equal_to(Reason::Off);

        let detail: Detail<AttributeValue> = Detail {
            value: None,
            variation_index: Some(1),
            reason: Reason::Off,
        };

        let or_detail = detail.or_else(|| false.into());
        assert_eq!(Some(AttributeValue::Bool(false)), or_detail.value);
        assert!(or_detail.variation_index.is_none());
        assert_that!(or_detail.reason).is_equal_to(Reason::Off);
    }
}
