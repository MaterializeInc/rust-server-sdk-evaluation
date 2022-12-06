#![cfg(test)]

use crate::flag::Flag;
use crate::segment::Segment;
use crate::store::Store;
use crate::PrerequisiteEvent;
use crate::PrerequisiteEventRecorder;
use maplit::hashmap;
use std::cell::RefCell;
use std::collections::HashMap;

pub struct TestStore {
    flags: HashMap<String, Flag>,
    segments: HashMap<String, Segment>,
}

impl TestStore {
    pub fn new() -> Self {
        Self {
            flags: hashmap! {
                "flag".to_string() => serde_json::from_str(r#"{
                        "key": "flag",
                        "version": 42,
                        "on": false,
                        "targets": [],
                        "rules": [],
                        "prerequisites": [],
                        "fallthrough": {"variation": 1},
                        "offVariation": 0,
                        "variations": [false, true],
                        "clientSide": true,
                        "clientSideAvailability": {
                            "usingEnvironmentId": true,
                            "usingMobileKey": true
                        },
                        "salt": "salty"
                    }"#).unwrap(),
                "flagWithRuleExclusion".to_string() => serde_json::from_str(r#"{
                        "key": "flag",
                        "version": 42,
                        "on": false,
                        "targets": [],
                        "rules": [
                            {
                                "variation": 0,
                                "id": "6a7755ac-e47a-40ea-9579-a09dd5f061bd",
                                "clauses": [
                                    {
                                        "attribute": "platform",
                                        "op": "in",
                                        "values": [
                                            "web",
                                            "aem",
                                            "ios"
                                        ],
                                        "negate": false
                                    }
                                ],
                                "trackEvents": true
                            }
                        ],
                        "prerequisites": [],
                        "fallthrough": {"variation": 1},
                        "offVariation": 0,
                        "variations": [false, true],
                        "clientSide": true,
                        "clientSideAvailability": {
                            "usingEnvironmentId": true,
                            "usingMobileKey": true
                        },
                        "salt": "salty",
                        "trackEvents": false,
                        "trackEventsFallthrough": true,
                        "debugEventsUntilDate": 1500000000
                    }"#).unwrap(),
                "flagWithMatchesOpOnGroups".to_string() => serde_json::from_str(r#"{
                        "key": "flagWithMatchesOpOnGroups",
                        "version": 42,
                        "on": true,
                        "targets": [],
                        "rules": [
                            {
                                "variation": 0,
                                "id": "6a7755ac-e47a-40ea-9579-a09dd5f061bd",
                                "clauses": [
                                    {
                                        "attribute": "groups",
                                        "op": "matches",
                                        "values": [
                                            "^\\w+"
                                        ],
                                        "negate": false
                                    }
                                ],
                                "trackEvents": true
                            }
                        ],
                        "prerequisites": [],
                        "fallthrough": {"variation": 1},
                        "offVariation": 0,
                        "variations": [false, true],
                        "clientSide": true,
                        "clientSideAvailability": {
                            "usingEnvironmentId": true,
                            "usingMobileKey": true
                        },
                        "salt": "salty",
                        "trackEvents": false,
                        "trackEventsFallthrough": true,
                        "debugEventsUntilDate": 1500000000
                    }"#).unwrap(),
                "flagWithMatchesOpOnKinds".to_string() => serde_json::from_str(r#"{
                        "key": "flagWithMatchesOpOnKinds",
                        "version": 42,
                        "on": true,
                        "targets": [],
                        "rules": [
                            {
                                "variation": 0,
                                "id": "6a7755ac-e47a-40ea-9579-a09dd5f061bd",
                                "clauses": [
                                    {
                                        "attribute": "kind",
                                        "op": "matches",
                                        "values": [
                                            "^[ou]"
                                        ],
                                        "negate": false
                                    }
                                ],
                                "trackEvents": true
                            }
                        ],
                        "prerequisites": [],
                        "fallthrough": {"variation": 1},
                        "offVariation": 0,
                        "variations": [false, true],
                        "clientSide": true,
                        "clientSideAvailability": {
                            "usingEnvironmentId": true,
                            "usingMobileKey": true
                        },
                        "salt": "salty",
                        "trackEvents": false,
                        "trackEventsFallthrough": true,
                        "debugEventsUntilDate": 1500000000
                    }"#).unwrap(),
                "flagWithMatchesOpOnKindsAttributeReference".to_string() => serde_json::from_str(r#"{
                        "key": "flagWithMatchesOpOnKindsAttributeReference",
                        "version": 42,
                        "on": true,
                        "targets": [],
                        "rules": [
                            {
                                "variation": 0,
                                "id": "6a7755ac-e47a-40ea-9579-a09dd5f061bd",
                                "clauses": [
                                    {
                                        "attribute": "/kind",
                                        "op": "matches",
                                        "values": [
                                            "^[ou]"
                                        ],
                                        "negate": false,
                                        "contextKind" : "arbitrary"
                                    }
                                ],
                                "trackEvents": true
                            }
                        ],
                        "prerequisites": [],
                        "fallthrough": {"variation": 1},
                        "offVariation": 0,
                        "variations": [false, true],
                        "clientSide": true,
                        "clientSideAvailability": {
                            "usingEnvironmentId": true,
                            "usingMobileKey": true
                        },
                        "salt": "salty",
                        "trackEvents": false,
                        "trackEventsFallthrough": true,
                        "debugEventsUntilDate": 1500000000
                    }"#).unwrap(),
                "flagWithMatchesOpOnKindsPlainAttributeReference".to_string() => serde_json::from_str(r#"{
                        "key": "flagWithMatchesOpOnKindsPlainAttributeReference",
                        "version": 42,
                        "on": true,
                        "targets": [],
                        "rules": [
                            {
                                "variation": 0,
                                "id": "6a7755ac-e47a-40ea-9579-a09dd5f061bd",
                                "clauses": [
                                    {
                                        "attribute": "kind",
                                        "op": "matches",
                                        "values": [
                                            "^[ou]"
                                        ],
                                        "negate": false,
                                        "contextKind" : "arbitrary"
                                    }
                                ],
                                "trackEvents": true
                            }
                        ],
                        "prerequisites": [],
                        "fallthrough": {"variation": 1},
                        "offVariation": 0,
                        "variations": [false, true],
                        "clientSide": true,
                        "clientSideAvailability": {
                            "usingEnvironmentId": true,
                            "usingMobileKey": true
                        },
                        "salt": "salty",
                        "trackEvents": false,
                        "trackEventsFallthrough": true,
                        "debugEventsUntilDate": 1500000000
                    }"#).unwrap(),
                "flagWithTrackAndDebugEvents".to_string() => serde_json::from_str(r#"{
                        "key": "flag",
                        "version": 42,
                        "on": false,
                        "targets": [],
                        "rules": [],
                        "prerequisites": [],
                        "fallthrough": {"variation": 1},
                        "offVariation": 0,
                        "variations": [false, true],
                        "clientSide": true,
                        "clientSideAvailability": {
                            "usingEnvironmentId": true,
                            "usingMobileKey": true
                        },
                        "salt": "salty",
                        "trackEvents": true,
                        "trackEventsFallthrough": true,
                        "debugEventsUntilDate": 1500000000
                    }"#).unwrap(),
                "flagWithExperiment".to_string() => serde_json::from_str(r#"{
                        "key": "flagWithExperiment",
                        "version": 42,
                        "on": true,
                        "targets": [],
                        "rules": [],
                        "prerequisites": [],
                        "fallthrough": {
                          "rollout": {
                            "kind": "experiment",
                            "seed": 61,
                            "variations": [
                              {"variation": 0, "weight": 10000, "untracked": false},
                              {"variation": 1, "weight": 20000, "untracked": false},
                              {"variation": 0, "weight": 70000, "untracked": true}
                            ]
                          }
                        },
                        "offVariation": 0,
                        "variations": [false, true],
                        "clientSide": true,
                        "clientSideAvailability": {
                            "usingEnvironmentId": true,
                            "usingMobileKey": true
                        },
                        "salt": "salty",
                        "trackEvents": false,
                        "trackEventsFallthrough": false,
                        "debugEventsUntilDate": 1500000000
                    }"#).unwrap(),
                "flagWithExperimentTargetingContext".to_string() => serde_json::from_str(r#"{
                        "key": "flagWithExperimentTargetingContext",
                        "version": 42,
                        "on": true,
                        "targets": [],
                        "rules": [],
                        "prerequisites": [],
                        "fallthrough": {
                          "rollout": {
                            "kind": "experiment",
                            "contextKind": "org",
                            "seed": 61,
                            "variations": [
                              {"variation": 0, "weight": 10000, "untracked": false},
                              {"variation": 1, "weight": 20000, "untracked": false},
                              {"variation": 0, "weight": 70000, "untracked": true}
                            ]
                          }
                        },
                        "offVariation": 0,
                        "variations": [false, true],
                        "clientSide": true,
                        "clientSideAvailability": {
                            "usingEnvironmentId": true,
                            "usingMobileKey": true
                        },
                        "salt": "salty",
                        "trackEvents": false,
                        "trackEventsFallthrough": false,
                        "debugEventsUntilDate": 1500000000
                    }"#).unwrap(),
                "flagWithRolloutBucketBy".to_string() => serde_json::from_str(r#"{
                        "key": "rollout",
                        "on": true,
                        "prerequisites": [],
                        "targets": [],
                        "rules": [
                            {
                                "rollout": {
                                    "variations": [
                                        {
                                            "variation": 0,
                                            "weight": 50000
                                        },
                                        {
                                            "variation": 1,
                                            "weight": 50000
                                        },
                                        {
                                            "variation": 2,
                                            "weight": 0
                                        }
                                    ],
                                    "bucketBy": "ld_quid"
                                },
                                "id": "6a7755ac-e47a-40ea-9579-a09dd5f061bd",
                                "clauses": [
                                    {
                                        "attribute": "platform",
                                        "op": "in",
                                        "values": [
                                            "web",
                                            "aem",
                                            "ios"
                                        ],
                                        "negate": false
                                    }
                                ],
                                "trackEvents": false
                            }
                        ],
                        "fallthrough": {
                            "variation": 2
                        },
                        "offVariation": 1,
                        "variations": [
                            "rollout1",
                            "rollout2",
                            "rollout3"
                        ],
                        "clientSide": true,
                        "clientSideAvailability": {
                            "usingMobileKey": true,
                            "usingEnvironmentId": true
                        },
                        "salt": "ce2634f116d741a7ad1b7ef363f6f9bc",
                        "trackEvents": false,
                        "trackEventsFallthrough": false,
                        "debugEventsUntilDate": null,
                        "version": 7
                    }"#).unwrap(),
                "flagWithTarget".to_string() => serde_json::from_str(r#"{
                        "key": "flagWithTarget",
                        "version": 42,
                        "on": false,
                        "targets": [{
                            "values": ["bob"],
                            "variation": 0
                        }],
                        "rules": [],
                        "prerequisites": [],
                        "fallthrough": {"variation": 1},
                        "offVariation": 0,
                        "variations": [false, true],
                        "clientSide": true,
                        "clientSideAvailability": {
                            "usingEnvironmentId": true,
                            "usingMobileKey": true
                        },
                        "salt": "salty"
                    }"#).unwrap(),
                "flagWithContextTarget".to_string() => serde_json::from_str(r#"{
                        "key": "flagWithContextTarget",
                        "version": 42,
                        "on": true,
                        "targets": [{
                            "values": ["bob"],
                            "variation": 1
                        }],
                        "contextTargets": [{
                            "contextKind": "org",
                            "values": ["LaunchDarkly"],
                            "variation": 1
                        }, {
                            "contextKind": "user",
                            "values": [],
                            "variation": 1
                        }],
                        "rules": [],
                        "prerequisites": [],
                        "fallthrough": {"variation": 0},
                        "offVariation": 0,
                        "variations": [false, true],
                        "clientSide": true,
                        "clientSideAvailability": {
                            "usingEnvironmentId": true,
                            "usingMobileKey": true
                        },
                        "salt": "salty"
                    }"#).unwrap(),
                "flagWithMissingPrereq".to_string() => serde_json::from_str(r#"{
                        "key": "flagWithMissingPrereq",
                        "version": 42,
                        "on": true,
                        "targets": [],
                        "rules": [],
                        "prerequisites": [{
                            "key": "badPrereq",
                            "variation": 1
                        }],
                        "fallthrough": {"variation": 1},
                        "offVariation": 0,
                        "variations": [false, true],
                        "clientSide": true,
                        "clientSideAvailability": {
                            "usingEnvironmentId": true,
                            "usingMobileKey": true
                        },
                        "salt": "salty"
                    }"#).unwrap(),
                "flagWithOffPrereq".to_string() => serde_json::from_str(r#"{
                        "key": "flagWithOffPrereq",
                        "version": 42,
                        "on": true,
                        "targets": [],
                        "rules": [],
                        "prerequisites": [{
                            "key": "offPrereq",
                            "variation": 1
                        }],
                        "fallthrough": {"variation": 1},
                        "offVariation": 0,
                        "variations": [false, true],
                        "clientSide": true,
                        "clientSideAvailability": {
                            "usingEnvironmentId": true,
                            "usingMobileKey": true
                        },
                        "salt": "salty"
                    }"#).unwrap(),
                "flagWithFirstPrereqAsPrereqToSecondPrereq".to_string() => serde_json::from_str(r#"{
                        "key": "flagWithFirstPrereqAsPrereqToSecondPrereq",
                        "version": 42,
                        "on": true,
                        "targets": [],
                        "rules": [],
                        "prerequisites": [{
                            "key": "prereq",
                            "variation": 1
                        },
                        {
                            "key": "flagWithSatisfiedPrereq",
                            "variation": 1
                        }],
                        "fallthrough": {"variation": 1},
                        "offVariation": 0,
                        "variations": [false, true],
                        "clientSide": true,
                        "clientSideAvailability": {
                            "usingEnvironmentId": true,
                            "usingMobileKey": true
                        },
                        "salt": "salty"
                    }"#).unwrap(),
                "flagWithNestedPrereq".to_string() => serde_json::from_str(r#"{
                        "key": "flagWithNestedPrereq",
                        "version": 42,
                        "on": true,
                        "targets": [],
                        "rules": [],
                        "prerequisites": [{
                            "key": "flagWithSatisfiedPrereq",
                            "variation": 1
                        }],
                        "fallthrough": {"variation": 1},
                        "offVariation": 0,
                        "variations": [false, true],
                        "clientSide": true,
                        "clientSideAvailability": {
                            "usingEnvironmentId": true,
                            "usingMobileKey": true
                        },
                        "salt": "salty"
                    }"#).unwrap(),
                "flagWithSatisfiedPrereq".to_string() => serde_json::from_str(r#"{
                        "key": "flagWithSatisfiedPrereq",
                        "version": 42,
                        "on": true,
                        "targets": [],
                        "rules": [],
                        "prerequisites": [{
                            "key": "prereq",
                            "variation": 1
                        }],
                        "fallthrough": {"variation": 1},
                        "offVariation": 0,
                        "variations": [false, true],
                        "clientSide": true,
                        "clientSideAvailability": {
                            "usingEnvironmentId": true,
                            "usingMobileKey": true
                        },
                        "salt": "salty"
                    }"#).unwrap(),
                "prereq".to_string() => serde_json::from_str(r#"{
                        "key": "prereq",
                        "version": 42,
                        "on": true,
                        "targets": [{
                            "values": ["bob"],
                            "variation": 0
                        }],
                        "rules": [],
                        "prerequisites": [],
                        "fallthrough": {"variation": 1},
                        "offVariation": 0,
                        "variations": [false, true],
                        "clientSide": true,
                        "clientSideAvailability": {
                            "usingEnvironmentId": true,
                            "usingMobileKey": true
                        },
                        "salt": "salty"
                    }"#).unwrap(),
                "offPrereq".to_string() => serde_json::from_str(r#"{
                        "key": "offPrereq",
                        "version": 42,
                        "on": false,
                        "targets": [],
                        "rules": [],
                        "prerequisites": [],
                        "fallthrough": {"variation": 1},
                        "offVariation": 1,
                        "variations": [false, true],
                        "clientSide": true,
                        "clientSideAvailability": {
                            "usingEnvironmentId": true,
                            "usingMobileKey": true
                        },
                        "salt": "salty"
                    }"#).unwrap(),
                "flagWithInRule".to_string() => serde_json::from_str(r#"{
                        "key": "flagWithInRule",
                        "version": 42,
                        "on": false,
                        "targets": [],
                        "rules": [{
                            "id": "in-rule",
                            "clauses": [{
                                "attribute": "team",
                                "negate": false,
                                "op": "in",
                                "values": ["Avengers"]
                            }],
                            "variation": 0,
                            "trackEvents": false
                        }],
                        "prerequisites": [],
                        "fallthrough": {"variation": 1},
                        "offVariation": 0,
                        "variations": [false, true],
                        "clientSide": true,
                        "clientSideAvailability": {
                            "usingEnvironmentId": true,
                            "usingMobileKey": true
                        },
                        "salt": "salty"
                    }"#).unwrap(),
                "flagWithSegmentMatchRule".to_string() => serde_json::from_str(r#"{
                        "key": "flagWithSegmentMatchRule",
                        "version": 42,
                        "on": true,
                        "targets": [],
                        "rules": [{
                            "id": "match-rule",
                            "clauses": [{
                                "contextKind": "user",
                                "attribute": "key",
                                "negate": false,
                                "op": "segmentMatch",
                                "values": ["segment"]
                            }],
                            "variation": 0,
                            "trackEvents": false
                        }],
                        "prerequisites": [],
                        "fallthrough": {"variation": 1},
                        "offVariation": 0,
                        "variations": [false, true],
                        "clientSide": true,
                        "clientSideAvailability": {
                            "usingEnvironmentId": true,
                            "usingMobileKey": true
                        },
                        "salt": "salty"
                    }"#).unwrap(),
                "flagWithPrereqWhichDuplicatesSegmentRuleCheck".to_string() => serde_json::from_str(r#"{
                        "key": "flagWithPrereqWhichDuplicatesSegmentRuleCheck",
                        "version": 42,
                        "on": true,
                        "targets": [],
                        "rules": [{
                            "id": "match-rule",
                            "clauses": [{
                                "contextKind": "user",
                                "attribute": "key",
                                "negate": false,
                                "op": "segmentMatch",
                                "values": ["segment"]
                            }],
                            "variation": 0,
                            "trackEvents": false
                        }],
                        "prerequisites": [{
                            "key": "flagWithSegmentMatchRule",
                            "variation": 0
                        }],
                        "fallthrough": {"variation": 1},
                        "offVariation": 0,
                        "variations": [false, true],
                        "clientSide": true,
                        "clientSideAvailability": {
                            "usingEnvironmentId": true,
                            "usingMobileKey": true
                        },
                        "salt": "salty"
                    }"#).unwrap(),
                "flagWithMalformedRule".to_string() => serde_json::from_str(r#"{
                        "key": "flagWithMalformedRule",
                        "version": 42,
                        "on": false,
                        "targets": [],
                        "rules": [{
                            "id": "in-rule",
                            "clauses": [{
                                "attribute": "key",
                                "negate": false,
                                "op": "in",
                                "values": ["yes"]
                            }],
                            "trackEvents": false
                        }],
                        "prerequisites": [],
                        "fallthrough": {"variation": 1},
                        "offVariation": 0,
                        "variations": [false, true],
                        "clientSide": true,
                        "clientSideAvailability": {
                            "usingEnvironmentId": true,
                            "usingMobileKey": true
                        },
                        "salt": "salty"
                    }"#).unwrap(),
            },
            segments: hashmap! {
                "segment".to_string() => serde_json::from_str(r#"{
                        "key": "segment",
                        "included": ["alice"],
                        "includedContexts": [{
                            "values": [],
                            "contextKind": "user"
                        }],
                        "excluded": [],
                        "rules": [],
                        "salt": "salty",
                        "version": 1
                    }"#).unwrap()
            },
        }
    }

    pub fn new_from_json_str(flag_json: &str, segment_json: &str) -> Self {
        let flags = serde_json::from_str(flag_json).unwrap();
        let segments = serde_json::from_str(segment_json).unwrap();

        Self { flags, segments }
    }

    pub fn update_flag(&mut self, flag_key: &str, fun: fn(&mut Flag) -> ()) {
        let flag = self.flags.get_mut(flag_key).unwrap();
        fun(flag);
    }
}

impl Store for TestStore {
    fn flag(&self, flag_key: &str) -> Option<Flag> {
        self.flags.get(flag_key).cloned()
    }

    fn segment(&self, segment_key: &str) -> Option<Segment> {
        self.segments.get(segment_key).cloned()
    }
}

pub struct InMemoryPrerequisiteEventRecorder {
    pub events: RefCell<Vec<PrerequisiteEvent>>,
}

impl PrerequisiteEventRecorder for InMemoryPrerequisiteEventRecorder {
    fn record(&self, event: PrerequisiteEvent) {
        self.events.borrow_mut().push(event);
    }
}
