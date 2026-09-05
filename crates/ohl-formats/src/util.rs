//! Shared bounds-checked casting helpers used by both `bsp30` and `wad3`.

use crate::error::{FormatError, Result};
use zerocopy::{FromBytes, Immutable, KnownLayout, Unaligned};

/// Casts `bytes` to a validated `&[T]`, rejecting a length that is not an
/// exact multiple of `size_of::<T>()` and any misalignment (`T` is
/// `Unaligned`, so alignment can never actually fail; the check exists so
/// this function's contract does not depend on that fact silently).
pub(crate) fn slice_of<T>(bytes: &[u8]) -> Result<&[T]>
where
    T: FromBytes + Immutable + KnownLayout + Unaligned,
{
    <[T]>::ref_from_bytes(bytes).map_err(|_| FormatError::SizeNotMultiple)
}

/// Casts the first `size_of::<T>()` bytes of `bytes` to `&T`, returning the
/// remaining bytes as well. `bytes` may be longer than `T`.
pub(crate) fn prefix_of<T>(bytes: &[u8]) -> Result<(&T, &[u8])>
where
    T: FromBytes + Immutable + KnownLayout + Unaligned,
{
    T::ref_from_prefix(bytes).map_err(|_| FormatError::Truncated)
}

/// Casts exactly `bytes` (no more, no less) to `&T`.
pub(crate) fn exact_of<T>(bytes: &[u8]) -> Result<&T>
where
    T: FromBytes + Immutable + KnownLayout + Unaligned,
{
    T::ref_from_bytes(bytes).map_err(|_| FormatError::Truncated)
}

/// Returns `data[offset..offset + length]`, rejecting an out-of-bounds or
/// overflowing range instead of panicking.
pub(crate) fn sub_slice(data: &[u8], offset: usize, length: usize) -> Result<&[u8]> {
    let end = offset.checked_add(length).ok_or(FormatError::OutOfBounds)?;
    data.get(offset..end).ok_or(FormatError::OutOfBounds)
}
