//! Bounded arithmetic helpers.
//!
//! Every operation here returns a [`SanitizedError`] instead of panicking or
//! silently wrapping on overflow/underflow, so parser and format code can
//! propagate a fixed, sanitized diagnostic instead of trusting
//! media-controlled sizes and counts.

use crate::error::SanitizedError;

/// Checked arithmetic that reports a [`SanitizedError`] instead of wrapping.
pub trait CheckedArithmetic: Sized + Copy {
    /// Adds `self` and `rhs`, returning [`SanitizedError::ArithmeticOverflow`]
    /// on overflow.
    fn checked_add_bounded(self, rhs: Self) -> Result<Self, SanitizedError>;
    /// Subtracts `rhs` from `self`, returning
    /// [`SanitizedError::ArithmeticUnderflow`] on underflow.
    fn checked_sub_bounded(self, rhs: Self) -> Result<Self, SanitizedError>;
    /// Multiplies `self` and `rhs`, returning
    /// [`SanitizedError::ArithmeticOverflow`] on overflow.
    fn checked_mul_bounded(self, rhs: Self) -> Result<Self, SanitizedError>;
}

macro_rules! impl_checked_arithmetic {
    ($($t:ty),+ $(,)?) => {
        $(
            impl CheckedArithmetic for $t {
                fn checked_add_bounded(self, rhs: Self) -> Result<Self, SanitizedError> {
                    self.checked_add(rhs).ok_or(SanitizedError::ArithmeticOverflow)
                }

                fn checked_sub_bounded(self, rhs: Self) -> Result<Self, SanitizedError> {
                    self.checked_sub(rhs).ok_or(SanitizedError::ArithmeticUnderflow)
                }

                fn checked_mul_bounded(self, rhs: Self) -> Result<Self, SanitizedError> {
                    self.checked_mul(rhs).ok_or(SanitizedError::ArithmeticOverflow)
                }
            }
        )+
    };
}

impl_checked_arithmetic!(
    u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize
);

/// Adds `a` and `b`, returning a sanitized error on overflow.
pub fn add<T: CheckedArithmetic>(a: T, b: T) -> Result<T, SanitizedError> {
    a.checked_add_bounded(b)
}

/// Subtracts `b` from `a`, returning a sanitized error on underflow.
pub fn sub<T: CheckedArithmetic>(a: T, b: T) -> Result<T, SanitizedError> {
    a.checked_sub_bounded(b)
}

/// Multiplies `a` and `b`, returning a sanitized error on overflow.
pub fn mul<T: CheckedArithmetic>(a: T, b: T) -> Result<T, SanitizedError> {
    a.checked_mul_bounded(b)
}

#[cfg(test)]
mod tests {
    use super::{add, mul, sub};
    use crate::error::SanitizedError;

    #[test]
    fn add_reports_overflow() {
        assert_eq!(add(1_u8, 2_u8), Ok(3));
        assert_eq!(add(u8::MAX, 1_u8), Err(SanitizedError::ArithmeticOverflow));
    }

    #[test]
    fn sub_reports_underflow() {
        assert_eq!(sub(5_u32, 2_u32), Ok(3));
        assert_eq!(sub(0_u32, 1_u32), Err(SanitizedError::ArithmeticUnderflow));
    }

    #[test]
    fn mul_reports_overflow() {
        assert_eq!(mul(3_u16, 4_u16), Ok(12));
        assert_eq!(
            mul(u16::MAX, 2_u16),
            Err(SanitizedError::ArithmeticOverflow)
        );
    }
}
