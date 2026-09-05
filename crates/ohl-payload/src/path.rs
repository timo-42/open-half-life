//! Cross-platform payload path policy.
//!
//! Archive-controlled names are the most hostile input an import handles: a
//! single accepted `..`, drive letter, reserved device name, or case-only
//! alias turns extraction into an arbitrary-write primitive. This module is
//! the one place that decides which archive-relative names may become
//! destination names, and it does so with a deliberately strict
//! common-denominator policy rather than a per-host one — a payload that
//! stages on Linux must be reproducible byte-for-byte on Windows and macOS.
//!
//! The rules, ported unchanged from the C++ `validate_payload_path`:
//!
//! - backslashes are separators and normalise to `/`;
//! - the whole path is at most [`MAXIMUM_PATH_BYTES`], each component at most
//!   [`MAXIMUM_COMPONENT_BYTES`], and there are at most
//!   [`MAXIMUM_COMPONENT_COUNT`] components;
//! - nothing rooted: no leading separator and no `x:` prefix in the second
//!   byte, which also rejects UNC spellings;
//! - no empty component, no `.`, no `..`;
//! - printable ASCII only (`0x20`–`0x7e`) minus `<>:"|?*`, with no trailing
//!   `.` or space;
//! - no Windows reserved device name, matched against the component's stem.
//!
//! ASCII-only is intentional and is *not* a placeholder for "we forgot about
//! Unicode": accepting non-ASCII requires a normalisation form and a case-fold
//! table to be part of the accepted policy, because otherwise two payloads
//! that a filesystem considers the same name pass collision detection. Until
//! that policy is defined and tested, a non-ASCII byte is
//! [`PayloadPathError::InvalidCharacter`].
//!
//! Validation produces a [`PayloadPath`], which carries both the normalised
//! spelling and an ASCII case-folded [`PayloadPath::portability_key`]. The key
//! is what [`crate::layout`] compares, so a case-insensitive host cannot be
//! handed two entries that collide only after extraction has begun.

use alloc::string::String;
use alloc::vec::Vec;

/// The largest accepted archive-relative path, in bytes.
pub const MAXIMUM_PATH_BYTES: usize = 4_096;

/// The largest accepted single path component, in bytes.
pub const MAXIMUM_COMPONENT_BYTES: usize = 255;

/// The largest accepted number of components in one path.
pub const MAXIMUM_COMPONENT_COUNT: usize = 32;

/// Bytes that are never accepted inside a component, on top of the printable
/// ASCII range check.
const FORBIDDEN_BYTES: &[u8] = b"<>:\"|?*";

/// Why an archive-relative path may not become a destination name.
///
/// Every variant is payload-free: it names the rule that refused the input and
/// never echoes the input itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PayloadPathError {
    /// The path had no bytes at all.
    Empty,
    /// The path exceeded [`MAXIMUM_PATH_BYTES`].
    TooLong,
    /// The path was absolute, drive-qualified, or a UNC spelling.
    Rooted,
    /// Two separators met, or the path ended with one.
    EmptyComponent,
    /// A component was `.` or `..`.
    Traversal,
    /// The path had more than [`MAXIMUM_COMPONENT_COUNT`] components.
    TooDeep,
    /// A component held a byte outside the accepted printable ASCII set, or
    /// ended with a `.` or a space.
    InvalidCharacter,
    /// A component exceeded [`MAXIMUM_COMPONENT_BYTES`].
    ComponentTooLong,
    /// A component's stem is a Windows reserved device name.
    ReservedName,
}

impl PayloadPathError {
    /// The fixed, payload-free message for this code.
    const fn message(self) -> &'static str {
        match self {
            Self::Empty => "payload path is empty",
            Self::TooLong => "payload path exceeds the accepted length",
            Self::Rooted => "payload path is not relative",
            Self::EmptyComponent => "payload path has an empty component",
            Self::Traversal => "payload path contains a traversal component",
            Self::TooDeep => "payload path has too many components",
            Self::InvalidCharacter => "payload path component has an unportable character",
            Self::ComponentTooLong => "payload path component exceeds the accepted length",
            Self::ReservedName => "payload path component is a reserved device name",
        }
    }
}

impl core::fmt::Display for PayloadPathError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl core::error::Error for PayloadPathError {}

impl From<PayloadPathError> for ohl_core::SanitizedError {
    fn from(_: PayloadPathError) -> Self {
        Self::InvalidInput
    }
}

/// A validated, normalised, archive-relative payload path.
///
/// Holding one of these is the proof that every rule in the [module
/// documentation](self) passed. Construction is the only way to obtain it, so
/// no later stage has to re-derive the policy.
///
/// The value is still only *lexically* safe. Extraction must additionally use
/// no-follow, create-new native calls; see [`crate::store`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PayloadPath {
    /// The normalised, slash-separated spelling.
    relative_path: String,
    /// The ASCII case-folded collision key for `relative_path`.
    portability_key: String,
}

impl PayloadPath {
    /// Validates and normalises one archive-relative path.
    ///
    /// # Errors
    ///
    /// See [`PayloadPathError`]. The first failing rule wins, in the order
    /// documented for the module, so the reported code is stable for a given
    /// input.
    pub fn parse(path: &str) -> Result<Self, PayloadPathError> {
        let bytes = path.as_bytes();
        if bytes.is_empty() {
            return Err(PayloadPathError::Empty);
        }
        if bytes.len() > MAXIMUM_PATH_BYTES {
            return Err(PayloadPathError::TooLong);
        }
        // A drive-qualified spelling is rejected wherever the colon sits in
        // the second byte, not only after an ASCII letter: `1:x` is no more
        // meaningful as a relative name than `C:x` is.
        if bytes[0] == b'/' || bytes[0] == b'\\' || (bytes.len() >= 2 && bytes[1] == b':') {
            return Err(PayloadPathError::Rooted);
        }

        let mut relative_path = String::with_capacity(path.len());
        let mut portability_key = String::with_capacity(path.len());
        let mut components = 0usize;
        for component in path.split(['/', '\\']) {
            components += 1;
            if components > MAXIMUM_COMPONENT_COUNT {
                return Err(PayloadPathError::TooDeep);
            }
            validate_component(component)?;
            if !relative_path.is_empty() {
                relative_path.push('/');
                portability_key.push('/');
            }
            relative_path.push_str(component);
            for byte in component.bytes() {
                portability_key.push(char::from(byte.to_ascii_lowercase()));
            }
        }

        Ok(Self {
            relative_path,
            portability_key,
        })
    }

    /// The normalised, slash-separated spelling.
    pub fn as_str(&self) -> &str {
        &self.relative_path
    }

    /// The ASCII case-folded key used to detect collisions before extraction.
    ///
    /// Two paths that a case-insensitive filesystem would map to one name
    /// share this key, so comparing keys rejects the alias while both names
    /// are still just strings.
    pub fn portability_key(&self) -> &str {
        &self.portability_key
    }

    /// The path's components, in order. Always at least one.
    pub fn components(&self) -> impl Iterator<Item = &str> {
        self.relative_path.split('/')
    }

    /// The path's components as owned strings, for handing to a store that
    /// resolves one component per native call.
    pub fn owned_components(&self) -> Vec<String> {
        self.components().map(String::from).collect()
    }

    /// The case-folded key and the spelling of every proper ancestor
    /// directory, shallowest first. Empty for a single-component path.
    pub(crate) fn ancestor_keys(&self) -> impl Iterator<Item = (&str, &str)> {
        self.portability_key.match_indices('/').map(|(offset, _)| {
            (
                &self.portability_key[..offset],
                &self.relative_path[..offset],
            )
        })
    }
}

impl core::fmt::Display for PayloadPath {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(&self.relative_path)
    }
}

/// Applies every per-component rule.
fn validate_component(component: &str) -> Result<(), PayloadPathError> {
    if component.is_empty() {
        return Err(PayloadPathError::EmptyComponent);
    }
    if component == "." || component == ".." {
        return Err(PayloadPathError::Traversal);
    }
    if component.len() > MAXIMUM_COMPONENT_BYTES {
        return Err(PayloadPathError::ComponentTooLong);
    }
    if component.ends_with('.') || component.ends_with(' ') {
        return Err(PayloadPathError::InvalidCharacter);
    }
    for byte in component.bytes() {
        if !(0x20..=0x7e).contains(&byte) || FORBIDDEN_BYTES.contains(&byte) {
            return Err(PayloadPathError::InvalidCharacter);
        }
    }
    if is_reserved_windows_name(component) {
        return Err(PayloadPathError::ReservedName);
    }
    Ok(())
}

/// Whether a component's stem is a Windows reserved device name.
///
/// Windows resolves `NUL.txt` to the `NUL` device, so the check runs against
/// the text before the first `.`, not against the whole component.
fn is_reserved_windows_name(component: &str) -> bool {
    const RESERVED: &[&str] = &["CON", "PRN", "AUX", "NUL", "CLOCK$", "CONIN$", "CONOUT$"];

    let stem = component.split('.').next().unwrap_or(component);
    if RESERVED
        .iter()
        .any(|reserved| stem.eq_ignore_ascii_case(reserved))
    {
        return true;
    }
    let bytes = stem.as_bytes();
    bytes.len() == 4
        && (stem[..3].eq_ignore_ascii_case("COM") || stem[..3].eq_ignore_ascii_case("LPT"))
        && bytes[3].is_ascii_digit()
}

#[cfg(test)]
mod tests {
    use super::{MAXIMUM_COMPONENT_COUNT, MAXIMUM_PATH_BYTES, PayloadPath, PayloadPathError};
    use alloc::string::{String, ToString as _};
    use alloc::vec::Vec;

    #[test]
    fn a_valid_path_is_normalized_portably() {
        let path = PayloadPath::parse("ProjectFixture\\Nested/AmberToken.dat").expect("valid");
        assert_eq!(path.as_str(), "ProjectFixture/Nested/AmberToken.dat");
        assert_eq!(
            path.portability_key(),
            "projectfixture/nested/ambertoken.dat"
        );
        assert_eq!(
            path.components().collect::<Vec<_>>(),
            ["ProjectFixture", "Nested", "AmberToken.dat"]
        );
        assert_eq!(path.owned_components().len(), 3);
        assert_eq!(path.to_string(), path.as_str());
    }

    #[test]
    fn every_unsafe_path_is_refused_with_its_own_code() {
        let deep = {
            let mut path = String::from("a");
            for _ in 0..MAXIMUM_COMPONENT_COUNT {
                path.push_str("/a");
            }
            path
        };
        let cases: &[(&str, PayloadPathError)] = &[
            ("", PayloadPathError::Empty),
            ("/absolute", PayloadPathError::Rooted),
            ("\\\\server\\share", PayloadPathError::Rooted),
            ("C:\\payload", PayloadPathError::Rooted),
            ("C:payload", PayloadPathError::Rooted),
            ("one//two", PayloadPathError::EmptyComponent),
            ("one/", PayloadPathError::EmptyComponent),
            ("./one", PayloadPathError::Traversal),
            ("one/../two", PayloadPathError::Traversal),
            ("bad:name", PayloadPathError::InvalidCharacter),
            ("nested/bad:name", PayloadPathError::InvalidCharacter),
            ("bad\nname", PayloadPathError::InvalidCharacter),
            ("bad*name", PayloadPathError::InvalidCharacter),
            ("trailing. ", PayloadPathError::InvalidCharacter),
            ("trailing.", PayloadPathError::InvalidCharacter),
            ("caf\u{e9}", PayloadPathError::InvalidCharacter),
            ("CON", PayloadPathError::ReservedName),
            ("nul.txt", PayloadPathError::ReservedName),
            ("Com9.cfg", PayloadPathError::ReservedName),
            ("com0", PayloadPathError::ReservedName),
            ("lpt1", PayloadPathError::ReservedName),
            ("LPT0.txt", PayloadPathError::ReservedName),
            ("conin$", PayloadPathError::ReservedName),
            ("ConOut$.log", PayloadPathError::ReservedName),
            ("clock$", PayloadPathError::ReservedName),
            (&deep, PayloadPathError::TooDeep),
        ];
        for (input, expected) in cases {
            assert_eq!(
                PayloadPath::parse(input).expect_err("refused"),
                *expected,
                "input classified wrongly"
            );
        }

        let long_component = "a".repeat(256);
        assert_eq!(
            PayloadPath::parse(&long_component).expect_err("refused"),
            PayloadPathError::ComponentTooLong
        );
        let long_path = "a".repeat(MAXIMUM_PATH_BYTES + 1);
        assert_eq!(
            PayloadPath::parse(&long_path).expect_err("refused"),
            PayloadPathError::TooLong
        );
    }

    #[test]
    fn the_boundary_lengths_and_depth_are_accepted() {
        let component = "a".repeat(255);
        assert!(PayloadPath::parse(&component).is_ok());

        let mut deep = String::from("a");
        for _ in 0..(MAXIMUM_COMPONENT_COUNT - 1) {
            deep.push_str("/a");
        }
        assert!(PayloadPath::parse(&deep).is_ok());

        // Sixteen 255-byte components plus fifteen separators is 4095 bytes,
        // which is inside every bound at once.
        let mut boundary = core::iter::repeat_n("a".repeat(255), 16)
            .collect::<Vec<_>>()
            .join("/");
        assert_eq!(boundary.len(), 4_095);
        assert!(PayloadPath::parse(&boundary).is_ok());

        // One more byte reaches MAXIMUM_PATH_BYTES but overruns a component.
        boundary.insert(0, 'b');
        assert_eq!(boundary.len(), MAXIMUM_PATH_BYTES);
        assert_eq!(
            PayloadPath::parse(&boundary).expect_err("refused"),
            PayloadPathError::ComponentTooLong
        );
    }

    #[test]
    fn a_case_only_alias_shares_its_collision_key() {
        let first = PayloadPath::parse("SyntheticBranch/CaseToken.DAT").expect("valid");
        let second = PayloadPath::parse("syntheticbranch/casetoken.dat").expect("valid");
        assert_eq!(first.portability_key(), second.portability_key());
        assert_ne!(first.as_str(), second.as_str());
    }

    #[test]
    fn ancestor_keys_walk_every_proper_parent() {
        let path = PayloadPath::parse("Alpha/Beta/Gamma.bin").expect("valid");
        assert_eq!(
            path.ancestor_keys().collect::<Vec<_>>(),
            [("alpha", "Alpha"), ("alpha/beta", "Alpha/Beta")]
        );
        let leaf = PayloadPath::parse("Alpha").expect("valid");
        assert_eq!(leaf.ancestor_keys().count(), 0);
    }

    proptest::proptest! {
        /// Normalisation is idempotent: reparsing a normalised path yields
        /// exactly the same value. Anything else would mean the identity a
        /// payload is published under could depend on how many times a path
        /// had been through the policy.
        #[test]
        fn normalization_is_idempotent(
            input in proptest::string::string_regex("[A-Za-z0-9_.\\\\/-]{0,64}")
                .expect("strategy")
        ) {
            let Ok(first) = PayloadPath::parse(&input) else {
                return Ok(());
            };
            let second = PayloadPath::parse(first.as_str()).expect("a normalised path reparses");
            proptest::prop_assert_eq!(first.as_str(), second.as_str());
            proptest::prop_assert_eq!(first.portability_key(), second.portability_key());
            // The key is exactly the ASCII case-folded spelling, so key
            // equality and case-insensitive name equality are the same thing.
            let folded = first.as_str().to_ascii_lowercase();
            proptest::prop_assert_eq!(first.portability_key(), folded.as_str());
            proptest::prop_assert!(!first.as_str().contains('\\'));
        }

        /// Two spellings collide under the policy exactly when a
        /// case-insensitive filesystem would treat them as one name.
        #[test]
        fn the_collision_key_detects_exactly_the_case_only_aliases(
            first in proptest::string::string_regex("[A-Za-z]{1,8}(/[A-Za-z]{1,8}){0,3}")
                .expect("strategy"),
            second in proptest::string::string_regex("[A-Za-z]{1,8}(/[A-Za-z]{1,8}){0,3}")
                .expect("strategy"),
        ) {
            let (Ok(first_path), Ok(second_path)) =
                (PayloadPath::parse(&first), PayloadPath::parse(&second))
            else {
                return Ok(());
            };
            proptest::prop_assert_eq!(
                first_path.portability_key() == second_path.portability_key(),
                first.eq_ignore_ascii_case(&second)
            );
        }
    }

    #[test]
    fn every_message_is_a_fixed_literal() {
        for error in [
            PayloadPathError::Empty,
            PayloadPathError::TooLong,
            PayloadPathError::Rooted,
            PayloadPathError::EmptyComponent,
            PayloadPathError::Traversal,
            PayloadPathError::TooDeep,
            PayloadPathError::InvalidCharacter,
            PayloadPathError::ComponentTooLong,
            PayloadPathError::ReservedName,
        ] {
            assert!(!error.to_string().is_empty());
            assert_eq!(
                ohl_core::SanitizedError::from(error),
                ohl_core::SanitizedError::InvalidInput
            );
        }
    }
}
