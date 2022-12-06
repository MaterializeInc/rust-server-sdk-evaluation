const FLOAT_TO_INT_MAX: f64 = 9007199254740991_f64;

/// Converting float to int has undefined behaviour for huge floats: https://stackoverflow.com/a/41139453.
/// To avoid this, refuse to convert floats with magnitude greater than 2**53 - 1, after which 64-bit floats no longer
/// retain integer precision. We could go a few orders of magnitude higher without triggering the UB, but this seems like
/// the least surprising place to put a breakpoint.
pub(crate) fn f64_to_i64_safe(f: f64) -> Option<i64> {
    if f.abs() <= FLOAT_TO_INT_MAX {
        Some(f as i64)
    } else {
        None
    }
}

pub(crate) fn is_false(b: &bool) -> bool {
    !(*b)
}
