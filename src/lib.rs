//! This crate contains the LaunchDarkly Rust SDK feature flag evaluation engine.
//!
//! Normal use of the Rust SDK does not require referencing this crate directly. It is used internally
//! by the SDK, but is published and versioned separately so it can be used in other LaunchDarkly
//! components without making the SDK versioning dependent on these internal APIs.

#![deny(rustdoc::missing_crate_level_docs)]
#![deny(missing_docs)]

mod attribute_value;
mod contexts;
mod eval;
mod flag;
mod flag_value;
mod rule;
mod segment;
mod store;
mod test_common;
mod util;
mod variation;

pub use attribute_value::AttributeValue;
pub use contexts::attribute_reference::Reference;
pub use contexts::context::{Context, ContextAttributes, Kind};
pub use contexts::context_builder::{ContextBuilder, MultiContextBuilder};
pub use eval::*;
pub use flag::*;
pub use flag_value::*;
pub use rule::*;
pub use segment::*;
pub use store::*;
pub use variation::*;

#[cfg(test)]
pub(crate) mod proptest_generators {
    pub(crate) use crate::contexts::attribute_reference::proptest_generators::*;
    pub(crate) use crate::contexts::context::proptest_generators::*;
    pub(crate) use crate::rule::proptest_generators::*;
    pub(crate) use crate::variation::proptest_generators::*;
}

/// Trait indicating that the item is versioned.
pub trait Versioned {
    /// Retrieve the version for this item instance.
    fn version(&self) -> u64;

    /// Determine if this item's version is greater than or equal to the provided version
    /// parameter.
    fn is_greater_than_or_equal(&self, version: u64) -> bool {
        self.version() >= version
    }
}
