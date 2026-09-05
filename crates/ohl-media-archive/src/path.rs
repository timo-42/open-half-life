//! Path rules shared by every media class.
//!
//! These are a direct port of the C++ facade's `normalize_path` and
//! `is_single_path_component`, and must stay behaviourally identical: the two
//! implementations guard the same trust boundary during the migration.

use alloc::string::String;

/// The longest path the readers accept, in bytes.
pub const MAX_PATH_BYTES: usize = 4_096;

/// Normalizes an untrusted path into an absolute, separator-canonical form.
///
/// Both `/` and `\` separate components, repeated separators collapse, and the
/// result always starts with `/`. `.` and `..` components, embedded NUL bytes,
/// and paths longer than [`MAX_PATH_BYTES`] are rejected: the readers never
/// resolve a relative or parent reference on the caller's behalf.
pub fn normalize_path(path: &str) -> Option<String> {
    if path.len() > MAX_PATH_BYTES || path.as_bytes().contains(&0) {
        return None;
    }

    let mut normalized = String::from("/");
    for component in path.split(['/', '\\']) {
        if component.is_empty() {
            continue;
        }
        if component == "." || component == ".." {
            return None;
        }
        if normalized.len() > 1 {
            normalized.push('/');
        }
        normalized.push_str(component);
    }
    Some(normalized)
}

/// Whether `name` is exactly one path component that may be looked up inside
/// an already-resolved directory.
pub fn is_single_path_component(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_PATH_BYTES
        && name != "."
        && name != ".."
        && !name.as_bytes().contains(&0)
        && !name.contains('/')
        && !name.contains('\\')
}

/// Splits a normalized path into its components. The root yields none.
pub fn components(normalized: &str) -> impl Iterator<Item = &str> {
    normalized.split('/').filter(|part| !part.is_empty())
}

#[cfg(test)]
mod tests {
    use super::{components, is_single_path_component, normalize_path};
    use alloc::string::ToString as _;

    #[test]
    fn matches_the_cxx_normalization_cases() {
        assert_eq!(normalize_path(""), Some("/".to_string()));
        assert_eq!(normalize_path("/"), Some("/".to_string()));
        assert_eq!(
            normalize_path("fixture-directory/fixture-file"),
            Some("/fixture-directory/fixture-file".to_string())
        );
        assert_eq!(
            normalize_path("//fixture-directory\\fixture-file"),
            Some("/fixture-directory/fixture-file".to_string())
        );
    }

    #[test]
    fn matches_the_cxx_rejection_cases() {
        assert_eq!(normalize_path("../fixture-file"), None);
        assert_eq!(normalize_path("fixture-directory/./fixture-file"), None);
        assert_eq!(normalize_path("fixture-directory/../fixture-file"), None);
        assert_eq!(normalize_path("safe\0hidden"), None);
        let too_long = "a".repeat(super::MAX_PATH_BYTES + 1);
        assert_eq!(normalize_path(&too_long), None);
    }

    #[test]
    fn single_components_match_the_cxx_rules() {
        assert!(is_single_path_component("fixture-file"));
        assert!(!is_single_path_component("../fixture-file"));
        assert!(!is_single_path_component("fixture-directory/fixture-file"));
        assert!(!is_single_path_component("//a\\b"));
        assert!(!is_single_path_component("safe\0hidden"));
        assert!(!is_single_path_component(""));
        assert!(!is_single_path_component("."));
        assert!(!is_single_path_component(".."));
    }

    #[test]
    fn components_of_the_root_are_empty() {
        assert_eq!(components("/").count(), 0);
        let parts: alloc::vec::Vec<_> = components("/a/b").collect();
        assert_eq!(parts, alloc::vec!["a", "b"]);
    }
}
