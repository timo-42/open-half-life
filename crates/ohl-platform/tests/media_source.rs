//! The Rust port of `tests/platform/media_source_test.cpp` and the
//! observable half of `media_source_native_security_test.cpp`.
//!
//! Every C++ case is represented: acquisition classification (missing,
//! directory, symlink, FIFO, device), empty files, exact and boundary reads,
//! huge offsets, zero-length reads, reads past the pinned end, pathname
//! replacement after pinning, truncation after pinning, in-place rewrite
//! detection, and concurrent positional reads through a shared capability.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ohl_platform::{MediaSource, MediaSourceError};

/// Builds the C++ fixture byte pattern: `(index * 37 + seed) & 0xff`.
fn make_bytes(size: usize, seed: u32) -> Vec<u8> {
    (0..size)
        .map(|index| {
            let index = u32::try_from(index).expect("fixture sizes stay small");
            u8::try_from(index.wrapping_mul(37).wrapping_add(seed) & 0xff)
                .expect("masked to a byte")
        })
        .collect()
}

/// Writes `bytes` to `path`, truncating any existing content.
fn write_bytes(path: &Path, bytes: &[u8]) -> PathBuf {
    let mut file = fs::File::create(path).expect("fixture creation");
    file.write_all(bytes).expect("fixture bytes");
    file.sync_all().expect("fixture flush");
    path.to_path_buf()
}

/// Reads `length` bytes at `offset` and returns them.
fn read_window(source: &MediaSource, offset: u64, length: usize) -> Vec<u8> {
    let mut actual = vec![0u8; length];
    source
        .read_exact_at(offset, &mut actual)
        .expect("in-range read");
    actual
}

#[test]
fn a_missing_path_is_not_found() {
    let root = tempfile::tempdir().expect("temporary directory");
    assert_eq!(
        MediaSource::open(&root.path().join("missing-source.fixture")).map(|_| ()),
        Err(MediaSourceError::NotFound)
    );
}

#[test]
fn a_directory_is_not_a_regular_file() {
    let root = tempfile::tempdir().expect("temporary directory");
    assert_eq!(
        MediaSource::open(root.path()).map(|_| ()),
        Err(MediaSourceError::NotRegularFile)
    );
}

#[test]
fn an_empty_path_is_not_found() {
    assert_eq!(
        MediaSource::open(Path::new("")).map(|_| ()),
        Err(MediaSourceError::NotFound)
    );
}

#[cfg(unix)]
#[test]
fn a_symbolic_link_in_the_final_component_is_rejected() {
    let root = tempfile::tempdir().expect("temporary directory");
    let target = root.path().join("link-target.fixture");
    write_bytes(&target, &make_bytes(97, 7));
    let link = root.path().join("selected-link.fixture");
    std::os::unix::fs::symlink("link-target.fixture", &link).expect("symlink creation");

    // The link resolves to a perfectly good regular file; acquisition must
    // still refuse it, because following it would defeat the whole point of
    // pinning the object the user actually selected.
    assert_eq!(
        MediaSource::open(&link).map(|_| ()),
        Err(MediaSourceError::NotRegularFile)
    );
}

// `mknodat` is a Linux-only entry point in `rustix`, so the FIFO case is
// gated to Linux; `a_socket_file_is_rejected` below covers the same
// non-regular-file rejection through a portable route on every Unix.
#[cfg(target_os = "linux")]
#[test]
fn a_fifo_is_rejected_without_blocking() {
    use rustix::fs::{CWD, FileType, Mode, mknodat};

    let root = tempfile::tempdir().expect("temporary directory");
    let path = root.path().join("synthetic-source.fifo");
    // A FIFO carries no device number, so zero is the correct `dev`.
    mknodat(CWD, &path, FileType::Fifo, Mode::RUSR | Mode::WUSR, 0)
        .expect("synthetic FIFO creation");

    // With no writer attached this would hang without O_NONBLOCK, so the test
    // completing at all is part of what it checks.
    assert_eq!(
        MediaSource::open(&path).map(|_| ()),
        Err(MediaSourceError::NotRegularFile)
    );
}

#[cfg(unix)]
#[test]
fn a_socket_file_is_rejected() {
    let root = tempfile::tempdir().expect("temporary directory");
    let path = root.path().join("synthetic-source.socket");
    // Binding a listener is the portable way to put a non-regular file into
    // the filesystem: it works on every Unix, unlike `mknod`.
    let listener =
        std::os::unix::net::UnixListener::bind(&path).expect("synthetic socket creation");

    assert_eq!(
        MediaSource::open(&path).map(|_| ()),
        Err(MediaSourceError::NotRegularFile)
    );
    drop(listener);
}

#[cfg(unix)]
#[test]
fn a_character_device_is_rejected() {
    assert_eq!(
        MediaSource::open(Path::new("/dev/null")).map(|_| ()),
        Err(MediaSourceError::NotRegularFile)
    );
}

#[test]
fn an_empty_source_reports_zero_and_accepts_only_the_empty_read() {
    let root = tempfile::tempdir().expect("temporary directory");
    let path = write_bytes(&root.path().join("empty.fixture"), &[]);
    let source = MediaSource::open(&path).expect("acquisition");

    assert_eq!(source.size(), 0);
    assert_eq!(source.verify_unchanged(), Ok(()));
    assert_eq!(source.read_exact_at(0, &mut []), Ok(()));
    assert_eq!(
        source.read_exact_at(1, &mut []),
        Err(MediaSourceError::OutOfRange)
    );
    assert_eq!(
        source.read_exact_at(0, &mut [0u8; 1]),
        Err(MediaSourceError::OutOfRange)
    );
}

#[test]
fn exact_and_boundary_reads_match_the_cpp_contract() {
    let root = tempfile::tempdir().expect("temporary directory");
    let bytes = make_bytes(257, 11);
    let path = write_bytes(&root.path().join("bounded.fixture"), &bytes);
    let source = MediaSource::open(&path).expect("acquisition");
    let size = u64::try_from(bytes.len()).expect("fixture size");

    assert_eq!(source.size(), size);
    assert_eq!(read_window(&source, 0, bytes.len()), bytes);
    assert_eq!(read_window(&source, 31, 73), bytes[31..104]);
    assert_eq!(read_window(&source, size - 1, 1), bytes[bytes.len() - 1..]);

    // Zero-length reads are valid at every offset through the pinned size.
    assert_eq!(source.read_exact_at(size, &mut []), Ok(()));
    assert_eq!(
        source.read_exact_at(size + 1, &mut []),
        Err(MediaSourceError::OutOfRange)
    );

    // Reads past the pinned end.
    assert_eq!(
        source.read_exact_at(size, &mut [0u8; 1]),
        Err(MediaSourceError::OutOfRange)
    );
    assert_eq!(
        source.read_exact_at(size - 1, &mut [0u8; 2]),
        Err(MediaSourceError::OutOfRange)
    );
}

#[test]
fn huge_offsets_cannot_overflow_the_window_check() {
    let root = tempfile::tempdir().expect("temporary directory");
    let bytes = make_bytes(257, 11);
    let path = write_bytes(&root.path().join("huge-offsets.fixture"), &bytes);
    let source = MediaSource::open(&path).expect("acquisition");

    assert_eq!(
        source.read_exact_at(u64::MAX, &mut [0u8; 1]),
        Err(MediaSourceError::OutOfRange)
    );
    assert_eq!(
        source.read_exact_at(u64::MAX, &mut []),
        Err(MediaSourceError::OutOfRange)
    );
    assert_eq!(
        source.read_exact_at(u64::MAX - 1, &mut [0u8; 4]),
        Err(MediaSourceError::OutOfRange)
    );
    assert_eq!(
        source.read_exact_at(u64::MAX / 2, &mut [0u8; 8]),
        Err(MediaSourceError::OutOfRange)
    );
}

#[test]
fn replacing_the_pathname_cannot_retarget_the_pinned_source() {
    let root = tempfile::tempdir().expect("temporary directory");
    let selected = root.path().join("selected.fixture");
    let original = make_bytes(193, 23);
    let replacement = make_bytes(original.len(), 97);
    write_bytes(&selected, &original);
    let source = MediaSource::open(&selected).expect("acquisition");

    fs::rename(&selected, root.path().join("selected-original.fixture")).expect("rename away");
    write_bytes(&selected, &replacement);

    // Still the originally selected object, byte for byte, and still
    // unchanged: replacing a *name* is not a change to the pinned *object*.
    assert_eq!(read_window(&source, 0, original.len()), original);
    assert_eq!(source.verify_unchanged(), Ok(()));
}

#[test]
fn truncating_the_pinned_object_is_detected() {
    let root = tempfile::tempdir().expect("temporary directory");
    let bytes = make_bytes(128, 31);
    let path = write_bytes(&root.path().join("truncated.fixture"), &bytes);
    let source = MediaSource::open(&path).expect("acquisition");

    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .expect("writer")
        .set_len(17)
        .expect("truncation");

    assert_eq!(
        source.verify_unchanged(),
        Err(MediaSourceError::Changed),
        "a truncation of the pinned object must be reported"
    );
    assert_eq!(
        source.read_exact_at(0, &mut vec![0u8; bytes.len()]),
        Err(MediaSourceError::UnexpectedEof),
        "reading the pinned size after truncation must report an early end"
    );
}

#[test]
fn an_in_place_rewrite_of_the_same_size_is_detected() {
    let root = tempfile::tempdir().expect("temporary directory");
    let bytes = make_bytes(128, 41);
    let path = write_bytes(&root.path().join("rewritten.fixture"), &bytes);
    let source = MediaSource::open(&path).expect("acquisition");
    let original_time = fs::metadata(&path)
        .expect("metadata")
        .modified()
        .expect("mtime");

    {
        use std::io::Seek as _;
        let mut writer = fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("writer");
        writer
            .seek(std::io::SeekFrom::Start(19))
            .expect("seek into the middle");
        writer.write_all(&[0xa5]).expect("in-place rewrite");
        writer.sync_all().expect("flush");
    }
    // Bump the change indicator explicitly: a same-size rewrite performed
    // within the filesystem's timestamp granularity would otherwise be
    // indistinguishable, exactly as the C++ test documented.
    let bumped = original_time + std::time::Duration::from_secs(2);
    fs::File::options()
        .write(true)
        .open(&path)
        .expect("writer")
        .set_times(fs::FileTimes::new().set_modified(bumped))
        .expect("timestamp bump");

    assert_eq!(source.verify_unchanged(), Err(MediaSourceError::Changed));
}

#[test]
fn appending_to_the_pinned_object_is_detected() {
    let root = tempfile::tempdir().expect("temporary directory");
    let bytes = make_bytes(64, 43);
    let path = write_bytes(&root.path().join("appended.fixture"), &bytes);
    let source = MediaSource::open(&path).expect("acquisition");

    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("writer")
        .write_all(b"more")
        .expect("append");

    assert_eq!(source.verify_unchanged(), Err(MediaSourceError::Changed));
    // The pinned size still bounds every read, so the appended bytes are
    // unreachable through this capability.
    assert_eq!(
        source.read_exact_at(u64::try_from(bytes.len()).expect("size"), &mut [0u8; 1]),
        Err(MediaSourceError::OutOfRange)
    );
}

#[test]
fn concurrent_positional_reads_through_a_shared_capability_agree() {
    const THREAD_COUNT: usize = 8;
    const ITERATIONS: usize = 512;
    const READ_SIZE: usize = 79;

    let root = tempfile::tempdir().expect("temporary directory");
    let bytes = make_bytes(64 * 1024, 67);
    let path = write_bytes(&root.path().join("concurrent.fixture"), &bytes);
    let source = Arc::new(MediaSource::open(&path).expect("acquisition"));
    let expected = Arc::new(bytes);

    std::thread::scope(|scope| {
        for thread_index in 0..THREAD_COUNT {
            let source = Arc::clone(&source);
            let expected = Arc::clone(&expected);
            scope.spawn(move || {
                let mut actual = [0u8; READ_SIZE];
                for iteration in 0..ITERATIONS {
                    let offset = (thread_index * 997 + iteration * 131)
                        % (expected.len() - actual.len() + 1);
                    source
                        .read_exact_at(u64::try_from(offset).expect("offset"), &mut actual)
                        .expect("concurrent read");
                    assert_eq!(actual[..], expected[offset..offset + READ_SIZE]);
                }
            });
        }
    });
}

#[test]
fn a_shared_capability_outlives_the_reference_that_opened_it() {
    let root = tempfile::tempdir().expect("temporary directory");
    let bytes = make_bytes(211, 53);
    let path = write_bytes(&root.path().join("shared.fixture"), &bytes);

    let surviving = {
        let source = Arc::new(MediaSource::open(&path).expect("acquisition"));
        let surviving = Arc::clone(&source);
        drop(source);
        surviving
    };
    assert_eq!(read_window(&surviving, 0, bytes.len()), bytes);
    assert_eq!(surviving.verify_unchanged(), Ok(()));
}

#[test]
fn deleting_the_pathname_does_not_disturb_the_pinned_source() {
    let root = tempfile::tempdir().expect("temporary directory");
    let bytes = make_bytes(101, 59);
    let path = write_bytes(&root.path().join("unlinked.fixture"), &bytes);
    let source = MediaSource::open(&path).expect("acquisition");

    fs::remove_file(&path).expect("unlink");

    assert_eq!(read_window(&source, 0, bytes.len()), bytes);
    assert_eq!(source.verify_unchanged(), Ok(()));
}
