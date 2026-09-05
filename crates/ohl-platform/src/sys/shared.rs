//! The platform-independent shape of a pinned-object snapshot.

/// One acquisition-time or verification-time observation of a pinned native
/// file object.
///
/// The fields are deliberately opaque integers so that the same comparison
/// logic — and the same unit tests — apply to every backend:
///
/// | Field | Unix | Windows |
/// |-------|------|---------|
/// | `identity.0` | `st_dev` | `dwVolumeSerialNumber` |
/// | `identity.1` | `st_ino` | `nFileIndexHigh:nFileIndexLow` |
/// | `change_stamp.0` | `st_mtime` seconds | `ftLastWriteTime` as 100 ns ticks |
/// | `change_stamp.1` | `st_mtime` nanoseconds | always `0` |
///
/// A snapshot never contains a path, a name, or any file content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeSnapshot {
    /// The stable native identity of the pinned object.
    pub(crate) identity: (u64, u64),
    /// The object's size in bytes at observation time.
    pub(crate) size_bytes: u64,
    /// The native content-change indicator at observation time.
    pub(crate) change_stamp: (i64, i64),
    /// Whether the object was still an ordinary regular file.
    pub(crate) is_regular_file: bool,
}

impl NativeSnapshot {
    /// Reports whether `self`, observed later, still describes exactly the
    /// object pinned at acquisition time (`acquired`).
    ///
    /// Type, identity, size, and the native change indicator must all agree.
    /// A non-regular current observation always fails, which is what turns a
    /// replaced-in-place object into a detected change rather than a silent
    /// read of unrelated bytes.
    pub(crate) fn matches_pin(&self, acquired: &Self) -> bool {
        self.is_regular_file
            && acquired.is_regular_file
            && self.identity == acquired.identity
            && self.size_bytes == acquired.size_bytes
            && self.change_stamp == acquired.change_stamp
    }
}

/// Windows-only classification logic, compiled everywhere so it can be
/// unit-tested on any host.
#[cfg_attr(
    not(windows),
    allow(
        dead_code,
        reason = "compiled on every host so the Windows rules stay testable"
    )
)]
pub(crate) mod windows_facts {
    use super::NativeSnapshot;

    /// The subset of `BY_HANDLE_FILE_INFORMATION` plus `GetFileType` that the
    /// Windows backend needs, in target-independent form.
    ///
    /// Keeping the conversion here rather than inside the `windows` backend lets
    /// the Windows classification rules be unit-tested on any host; the backend
    /// itself only copies fields out of the native structure.
    #[derive(Debug, Clone, Copy)]
    pub(crate) struct WindowsFileFacts {
        /// Whether `GetFileType` reported `FILE_TYPE_DISK`.
        pub(crate) is_disk: bool,
        /// `dwFileAttributes`.
        pub(crate) attributes: u32,
        /// `dwVolumeSerialNumber`.
        pub(crate) volume_serial: u32,
        /// `nFileIndexHigh:nFileIndexLow` recombined.
        pub(crate) file_index: u64,
        /// `nFileSizeHigh:nFileSizeLow` recombined.
        pub(crate) size_bytes: u64,
        /// `ftLastWriteTime` as 100 ns ticks.
        pub(crate) last_write_ticks: u64,
    }

    /// `FILE_ATTRIBUTE_DIRECTORY`.
    pub(crate) const WINDOWS_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;
    /// `FILE_ATTRIBUTE_DEVICE`.
    pub(crate) const WINDOWS_ATTRIBUTE_DEVICE: u32 = 0x0000_0040;
    /// `FILE_ATTRIBUTE_REPARSE_POINT`.
    pub(crate) const WINDOWS_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

    impl WindowsFileFacts {
        /// Attributes that disqualify an object from being an ordinary file.
        ///
        /// Rejecting `FILE_ATTRIBUTE_REPARSE_POINT` here is what makes
        /// `FILE_FLAG_OPEN_REPARSE_POINT` a no-follow acquisition: the link
        /// itself was opened, recognised, and refused.
        const EXCLUDED_ATTRIBUTES: u32 = WINDOWS_ATTRIBUTE_DIRECTORY
            | WINDOWS_ATTRIBUTE_DEVICE
            | WINDOWS_ATTRIBUTE_REPARSE_POINT;

        /// Converts native facts into the portable snapshot shape.
        #[allow(
            clippy::cast_possible_wrap,
            reason = "FILETIME ticks are bounded by year 30828 and always fit in i64"
        )]
        pub(crate) fn into_snapshot(self) -> NativeSnapshot {
            NativeSnapshot {
                identity: (u64::from(self.volume_serial), self.file_index),
                size_bytes: self.size_bytes,
                change_stamp: (self.last_write_ticks as i64, 0),
                is_regular_file: self.is_disk && (self.attributes & Self::EXCLUDED_ATTRIBUTES) == 0,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::NativeSnapshot;
    use super::windows_facts::{
        WINDOWS_ATTRIBUTE_DEVICE, WINDOWS_ATTRIBUTE_DIRECTORY, WINDOWS_ATTRIBUTE_REPARSE_POINT,
        WindowsFileFacts,
    };

    fn snapshot() -> NativeSnapshot {
        NativeSnapshot {
            identity: (0x10, 0x20),
            size_bytes: 4096,
            change_stamp: (1_700_000_000, 123),
            is_regular_file: true,
        }
    }

    #[test]
    fn an_identical_observation_matches() {
        assert!(snapshot().matches_pin(&snapshot()));
    }

    #[test]
    fn a_different_device_or_inode_is_a_replacement() {
        let mut current = snapshot();
        current.identity.0 += 1;
        assert!(!current.matches_pin(&snapshot()));

        let mut current = snapshot();
        current.identity.1 += 1;
        assert!(!current.matches_pin(&snapshot()));
    }

    #[test]
    fn a_smaller_size_is_a_truncation() {
        let mut current = snapshot();
        current.size_bytes = 17;
        assert!(!current.matches_pin(&snapshot()));
    }

    #[test]
    fn a_larger_size_is_an_append() {
        let mut current = snapshot();
        current.size_bytes += 1;
        assert!(!current.matches_pin(&snapshot()));
    }

    #[test]
    fn a_same_size_rewrite_is_caught_by_the_change_stamp() {
        let mut current = snapshot();
        current.change_stamp.1 += 1;
        assert!(!current.matches_pin(&snapshot()));

        let mut current = snapshot();
        current.change_stamp.0 += 2;
        assert!(!current.matches_pin(&snapshot()));
    }

    #[test]
    fn a_no_longer_regular_object_never_matches() {
        let mut current = snapshot();
        current.is_regular_file = false;
        assert!(!current.matches_pin(&snapshot()));
    }

    #[test]
    fn windows_facts_become_a_portable_snapshot() {
        let facts = WindowsFileFacts {
            is_disk: true,
            attributes: 0x0000_0080,
            volume_serial: 0xdead_beef,
            file_index: 0x0000_0001_0000_0002,
            size_bytes: 4096,
            last_write_ticks: 133_000_000_000_000_000,
        };
        let snapshot = facts.into_snapshot();
        assert_eq!(snapshot.identity, (0xdead_beef, 0x0000_0001_0000_0002));
        assert_eq!(snapshot.size_bytes, 4096);
        assert_eq!(snapshot.change_stamp, (133_000_000_000_000_000, 0));
        assert!(snapshot.is_regular_file);
    }

    #[test]
    fn windows_directories_devices_and_reparse_points_are_not_regular() {
        for attribute in [
            WINDOWS_ATTRIBUTE_DIRECTORY,
            WINDOWS_ATTRIBUTE_DEVICE,
            WINDOWS_ATTRIBUTE_REPARSE_POINT,
        ] {
            let facts = WindowsFileFacts {
                is_disk: true,
                attributes: 0x0000_0080 | attribute,
                volume_serial: 1,
                file_index: 2,
                size_bytes: 0,
                last_write_ticks: 0,
            };
            assert!(!facts.into_snapshot().is_regular_file);
        }
    }

    #[test]
    fn a_non_disk_windows_handle_is_not_regular() {
        let facts = WindowsFileFacts {
            is_disk: false,
            attributes: 0x0000_0080,
            volume_serial: 1,
            file_index: 2,
            size_bytes: 0,
            last_write_ticks: 0,
        };
        assert!(!facts.into_snapshot().is_regular_file);
    }
}
