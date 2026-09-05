//! Property tests for the provenance manifest codec.
//!
//! The generated values are project-authored: random digests, sizes, and
//! printable-ASCII labels. No generator reads or approximates real media.

use ohl_media::{
    CacheManifest, MANIFEST_SCHEMA_VERSION, MAXIMUM_MANIFEST_BYTES, MediaClass, MediaDigest,
    PAYLOAD_STATE_NOT_IMPORTED, VolumeLabel,
};
use proptest::prelude::*;

/// Any printable-ASCII label that fits the bound.
fn label_strategy() -> impl Strategy<Value = VolumeLabel> {
    proptest::string::string_regex("[ -~]{0,32}")
        .expect("valid label pattern")
        .prop_map(|text| VolumeLabel::new(&text).expect("printable and bounded"))
}

fn class_strategy() -> impl Strategy<Value = MediaClass> {
    prop_oneof![Just(MediaClass::Udf), Just(MediaClass::Iso9660)]
}

fn manifest_strategy() -> impl Strategy<Value = CacheManifest> {
    (
        any::<[u8; 32]>(),
        any::<u64>(),
        class_strategy(),
        label_strategy(),
        label_strategy(),
        any::<u64>(),
    )
        .prop_map(
            |(digest, size_bytes, class, filesystem, label, created_unix_seconds)| CacheManifest {
                schema_version: MANIFEST_SCHEMA_VERSION,
                digest: MediaDigest::from_bytes(digest),
                size_bytes,
                class,
                filesystem,
                label,
                created_unix_seconds,
                payload_state: VolumeLabel::new(PAYLOAD_STATE_NOT_IMPORTED).expect("printable"),
            },
        )
}

proptest! {
    /// Serializing and parsing a manifest is lossless for every field.
    #[test]
    fn a_manifest_round_trips(manifest in manifest_strategy()) {
        let json = manifest.to_json().expect("serialized");
        prop_assert_eq!(CacheManifest::parse(json.as_bytes()).expect("parsed"), manifest);
    }

    /// Every manifest this build can produce stays inside the bound that the
    /// reader enforces, so a legitimate entry can never be refused as
    /// oversized.
    #[test]
    fn a_manifest_always_fits_the_bounded_size(manifest in manifest_strategy()) {
        let json = manifest.to_json().expect("serialized");
        prop_assert!(u64::try_from(json.len()).expect("length") <= MAXIMUM_MANIFEST_BYTES);
    }

    /// A manifest is always printable ASCII on a fixed number of lines: an
    /// opening brace, exactly the eight documented fields, and a closing
    /// brace. A label may itself contain a slash, so the absence of a *path*
    /// is asserted structurally rather than by scanning for separators.
    #[test]
    fn a_manifest_is_printable_ascii_with_exactly_the_documented_fields(
        manifest in manifest_strategy(),
    ) {
        let json = manifest.to_json().expect("serialized");
        prop_assert!(json.is_ascii());
        for character in json.chars().filter(|character| *character != '\n') {
            prop_assert!(!character.is_control(), "control character in manifest");
        }
        prop_assert_eq!(json.lines().count(), 10, "unexpected manifest field count");
    }

    /// A manifest whose declared schema is not this build's is rejected by
    /// the schema code, never parsed as if it were understood.
    #[test]
    fn a_foreign_schema_version_is_always_rejected(
        version in any::<u32>().prop_filter(
            "must differ from this build",
            |version| *version != MANIFEST_SCHEMA_VERSION,
        ),
        manifest in manifest_strategy(),
    ) {
        let mut foreign = manifest;
        foreign.schema_version = version;
        let json = foreign.to_json().expect("serialized");
        prop_assert_eq!(
            CacheManifest::parse(json.as_bytes()).expect_err("foreign schema"),
            ohl_media::ImportCacheError::ManifestSchemaUnsupported
        );
    }

    /// Arbitrary bytes are either parsed into a manifest or refused with a
    /// fixed code; parsing never panics and never allocates unboundedly.
    #[test]
    fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..8_192)) {
        let _ = CacheManifest::parse(&bytes);
    }
}
