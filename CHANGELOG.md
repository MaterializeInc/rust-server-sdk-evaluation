# Change log

All notable changes to the project will be documented in this file. This project adheres to [Semantic Versioning](http://semver.org).

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
