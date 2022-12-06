use crate::contexts::context_serde::Meta;
use serde::Deserialize;

/// If a bool is false, there is no need to serialize it in
/// a context JSON representation.
pub(super) fn is_false_bool_option(b: &Option<bool>) -> bool {
    match b {
        None => true,
        Some(b) => !b,
    }
}

/// If a vector is empty, there is no need to serialize it in
/// a context JSON representation.
pub(super) fn is_empty_vec_option<'re, T>(v: &Option<Vec<T>>) -> bool
where
    T: Deserialize<'re>,
{
    match v {
        None => true,
        Some(v) => v.is_empty(),
    }
}

/// If the _meta block's members are none, then there is no
/// need to serialize it in the context JSON representation.
pub(super) fn is_none_meta_option(m: &Option<Meta>) -> bool {
    match m {
        None => true,
        Some(m) => m.secondary.is_none() && is_empty_vec_option(&m.private_attributes),
    }
}
