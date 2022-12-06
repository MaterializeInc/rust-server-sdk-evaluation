# Change log

All notable changes to the project will be documented in this file. This project adheres to [Semantic Versioning](http://semver.org).

## [1.0.0] - 2022-12-06
This release of the evaluation engine corresponds to the upcoming v1.0.0 release of the LaunchDarkly server-side Rust SDK (launchdarkly-server-sdk), and is not compatible with earlier SDK versions.

### Added:
- Added the types `Context`, `Kind`, `ContextBuilder`, `MultiContextBuilder`, `Reference`, and supporting types. `Context` defines the new context-based evaluation model. `Context` replace `User`, which was removed.
- Added: `urlencoding` @ `2.1.0`
- Added: `maplit` @ `1.0.1`
- Added: `itertools` @ `0.10.3`
- Added: `serde_with` @ `2.1.0`


### Changed:
- The [MSRV](https://rust-lang.github.io/rfcs/2495-min-rust-version.html) is now 1.60.0
- Updated: `chrono` from `0.4.15` to `0.4.23`; only enable the `"std"` feature
- Updated: `semver` from `0.10.0` to `1.0.14`
- `evaluate` now takes a `Context` instead of a `User`
- User keys could previously be empty strings. With contexts, the key cannot be empty. 
- The "secondary" meta-attribute which affected percentage rollouts has been removed. If you set an attribute with that name in a context, it will be a custom attribute like any other.
- For backwards-compatibility, it is possible to enable "secondary" evaluation logic within the evaluation engine by enabling the `secondary_key_bucketing` flag. This will only affect contexts that were created via deserialization, since it is not possible to set "secondary" via a builder method. 

### Removed:
- `User`, `UserBuilder`, `UserAttributes`. See `Context`, `ContextBuilder` and `MultiContextBuilder` instead.

## [1.0.0-beta.5] - 2022-04-04
### Changed
- Update to edition 2021.
- Change store signature to support persistent stores.

### Added
- Support flag serialization of old and current schema.
- Add versioned trait (implemented by flag and segment).

## [1.0.0-beta.4] - 2022-03-07
### Changed
- Bump sha1 and test\_case dependencies.

### Fixed
- When serializing a user, custom attributes were incorrectly being flattened
  in the JSON output. We now correctly encode them under a `custom` key.

## [1.0.0-beta.3] - 2022-02-16
### Added
- Add support for globally and user specific private attributes.

### Changed
- If we receive a negative variation index, or an unsupported operation, the
  SDK should not fail to parse the payload.

## [1.0.0-beta.2] - 2022-01-21
### Changed
- Modified the `try_map` function to accept a default value when returning a new Detail instance.

## [1.0.0-beta.1] - 2022-01-19
Initial release of flag evaluation support code that will be used with the LaunchDarkly Server-Side SDK for Rust.
