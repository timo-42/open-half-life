//! Sanitized diagnostics.
//!
//! `SanitizedError` is the one error type shared across the Rust port that is
//! guaranteed to never carry media-derived or otherwise untrusted bytes: every
//! variant is a fixed, project-defined code whose `Display` implementation is
//! a fixed string literal. Call sites that need to preserve additional
//! context must record it out of band (for example in `tracing` fields marked
//! for redaction), never by formatting it into this type.

use thiserror::Error;

/// A sanitized, non-secret diagnostic code.
///
/// Every variant carries no payload, so no formatting implementation on this
/// type can ever interpolate caller- or media-supplied data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Error)]
#[non_exhaustive]
pub enum SanitizedError {
    /// A bounded arithmetic operation would have overflowed its type.
    #[error("bounded arithmetic overflowed")]
    ArithmeticOverflow,
    /// A bounded arithmetic operation would have underflowed its type.
    #[error("bounded arithmetic underflowed")]
    ArithmeticUnderflow,
    /// Input failed a validation check.
    #[error("input failed validation")]
    InvalidInput,
    /// A requested resource was not found.
    #[error("requested resource was not found")]
    NotFound,
    /// The requested operation is not supported in this build.
    #[error("operation is not supported")]
    Unsupported,
    /// An internal invariant was violated; this indicates a bug.
    #[error("internal invariant violated")]
    Internal,
}

#[cfg(test)]
mod tests {
    use super::SanitizedError;

    #[test]
    fn display_is_fixed_and_stable() {
        assert_eq!(
            SanitizedError::ArithmeticOverflow.to_string(),
            "bounded arithmetic overflowed"
        );
        assert_eq!(
            SanitizedError::NotFound.to_string(),
            "requested resource was not found"
        );
    }

    #[test]
    fn error_is_copy_and_comparable() {
        let a = SanitizedError::InvalidInput;
        let b = a;
        assert_eq!(a, b);
    }
}
