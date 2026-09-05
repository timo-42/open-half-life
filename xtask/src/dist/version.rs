//! Release version resolution, matching `crates/ohl-app/build.rs`:
//! `OHL_VERSION` from CI's `vergit` step when set, else the crate's own
//! `CARGO_PKG_VERSION` for local developer builds.

/// Resolves the version to package from an already-read `OHL_VERSION`
/// (`None` when unset or not valid UTF-8) and a fallback package version.
///
/// Split out from [`resolve`] so the fallback behaviour is testable without
/// mutating process-wide environment state (`std::env::set_var` is an
/// `unsafe fn` as of the 2024 edition, and this workspace forbids
/// `unsafe_code` outright).
#[must_use]
pub fn resolve_with(ohl_version: Option<&str>, package_version: &str) -> String {
    ohl_version
        .filter(|value| !value.is_empty())
        .unwrap_or(package_version)
        .to_owned()
}

/// Resolves the version to package, preferring `OHL_VERSION` (set by CI, see
/// `crates/ohl-app/build.rs` and `crates/ohl-core/build.rs` for the same
/// pattern used to stamp the binary itself) and falling back to this crate's
/// own `CARGO_PKG_VERSION`.
#[must_use]
pub fn resolve() -> String {
    resolve_with(
        std::env::var("OHL_VERSION").ok().as_deref(),
        env!("CARGO_PKG_VERSION"),
    )
}

#[cfg(test)]
mod tests {
    use super::resolve_with;

    #[test]
    fn falls_back_to_the_package_version_when_unset() {
        assert_eq!(resolve_with(None, "0.1.0"), "0.1.0");
    }

    #[test]
    fn falls_back_to_the_package_version_when_empty() {
        assert_eq!(resolve_with(Some(""), "0.1.0"), "0.1.0");
    }

    #[test]
    fn prefers_ohl_version_when_set() {
        assert_eq!(resolve_with(Some("9.9.9-test"), "0.1.0"), "9.9.9-test");
    }
}
