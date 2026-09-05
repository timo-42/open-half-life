//! End-to-end lifetime of the real, confined media-parser worker.
//!
//! The whole file is gated on Linux x86-64: on macOS and Windows there is no
//! isolated-worker backend and no freestanding image, so `cargo test
//! --workspace` compiles this to nothing and skips it.
//!
//! Everything here goes through the *production* launcher,
//! `ohl_platform::launch_isolated_worker`, which resolves the image at
//! `<directory of the current executable>/libexec/open-half-life/`
//! `ohl-media-parser-worker`. A cargo test binary lives in
//! `target/<profile>/deps`, and installing an image there would change what
//! every other test binary in the workspace resolves, so
//! `the_confined_worker_scenarios_pass_in_a_staged_layout` instead stages a
//! private directory, copies this test binary into it, installs the image
//! beside the copy, and re-runs the `#[ignore]`d scenarios below in that
//! child. The scenarios refuse to run anywhere else.
//!
//! The launcher deliberately reduces a child's exit status to the fixed
//! `IsolatedWorkerExitKind` vocabulary, so these scenarios assert `Clean`
//! versus `Failed`; the numeric statuses are pinned by
//! `ohl_parser_worker::contract` and documented in the crate README.

#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use ohl_parser_protocol::messages::{
    HELLO_PAYLOAD_BYTES, decode_ready_payload, encode_hello_payload, encode_stream_entry_payload,
};
use ohl_parser_protocol::{
    FRAME_HEADER_BYTES, FrameHeader, FrameView, Hello, MessageType, StreamEntry,
    decode_frame_header, encode_frame,
};
use ohl_parser_worker::{
    IMAGE_NAME, build_parser_worker_image, install_parser_worker_image, set_trusted_directory_mode,
};
use ohl_platform::{
    IsolatedWorker, IsolatedWorkerCancellationToken, IsolatedWorkerError, IsolatedWorkerExitKind,
    IsolatedWorkerService, launch_isolated_worker,
};

const SESSION_ID: u64 = 0x1122_3344_5566_7788;
const SOURCE_SIZE: u64 = 4096;
const MAXIMUM_READ_BYTES: u32 = 256;
const STREAM_ENTRY_PAYLOAD_BYTES: usize = 8;

/// Set by the staging test on the child it re-runs. Without it the scenarios
/// have no image beside them and would only prove the resolver fails.
const STAGED_MARKER: &str = "OHL_PARSER_WORKER_STAGED_LAYOUT";

/// How many `#[ignore]`d scenarios the staged child must run.
const SCENARIO_COUNT: usize = 5;

// -------------------------------------------------------------- staging ----

fn staging_directory() -> PathBuf {
    let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(
        || {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .and_then(Path::parent)
                .expect("crates/ohl-parser-worker has a grandparent")
                .join("target")
        },
        PathBuf::from,
    );
    target
        .join("ohl-parser-worker-image")
        .join(format!("lifetime-{}", std::process::id()))
}

fn copy_executable(destination: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt as _;
    let source = std::env::current_exe().expect("the test binary has a path");
    let copy = destination.join("worker-lifetime-host");
    std::fs::copy(&source, &copy).expect("the test binary can be copied");
    std::fs::set_permissions(&copy, std::fs::Permissions::from_mode(0o755))
        .expect("the copy is executable");
    copy
}

/// Stages a private install layout and re-runs every `#[ignore]`d scenario in
/// a copy of this binary that lives beside the image.
#[test]
fn the_confined_worker_scenarios_pass_in_a_staged_layout() {
    let directory = staging_directory();
    std::fs::create_dir_all(&directory).expect("the staging directory is creatable");
    // The backend refuses any group- or world-writable component on the
    // image's resolution path, which is what an ambient `umask 002` produces.
    set_trusted_directory_mode(&directory).expect("the staging directory mode is settable");

    let image = build_parser_worker_image().expect("the media-parser worker image builds");
    let installed = install_parser_worker_image(&image, &directory)
        .expect("the image installs beside the copy");
    assert_eq!(
        installed,
        directory
            .join("libexec")
            .join("open-half-life")
            .join(IMAGE_NAME)
    );

    let host = copy_executable(&directory);
    let output = Command::new(&host)
        .args(["--ignored", "--test-threads", "1", "--nocapture"])
        .env(STAGED_MARKER, "1")
        .output()
        .expect("the staged host runs");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "staged worker scenarios failed\n--- stdout ---\n{stdout}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Prove the scenarios ran rather than being skipped: the child must
    // report every one of them as passed, not ignored or filtered out.
    assert!(
        stdout.contains(&format!("{SCENARIO_COUNT} passed")),
        "the staged host did not run every scenario\n{stdout}"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

/// Whether this process is the staged child. The scenarios below are
/// `#[ignore]`d, so a plain `cargo test -- --ignored` outside the staged
/// layout skips them instead of failing on a missing image.
fn staged() -> bool {
    std::env::var_os(STAGED_MARKER).is_some()
}

// ------------------------------------------------------ protocol driver ----

fn deadline(after: Duration) -> Instant {
    Instant::now().checked_add(after).expect("a near deadline")
}

fn none() -> IsolatedWorkerCancellationToken {
    IsolatedWorkerCancellationToken::default()
}

/// Launches a confined worker through the production resolver. Reaching this
/// point already proves the worker wrote the readiness attestation on
/// descriptor 4 and closed it: the launcher refuses to return otherwise.
fn launch() -> IsolatedWorker {
    launch_isolated_worker(
        IsolatedWorkerService::MediaParser,
        deadline(Duration::from_secs(10)),
    )
    .expect("a confined media-parser worker launches")
}

fn frame(header: &FrameHeader, payload: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0_u8; FRAME_HEADER_BYTES + payload.len()];
    let written = encode_frame(header, payload, &mut bytes).expect("frame encoding");
    assert_eq!(written, bytes.len());
    bytes
}

fn hello_frame() -> Vec<u8> {
    let mut payload = [0_u8; HELLO_PAYLOAD_BYTES];
    encode_hello_payload(
        &Hello {
            source_size: SOURCE_SIZE,
            maximum_read_bytes: MAXIMUM_READ_BYTES,
        },
        &mut payload,
    )
    .expect("hello encoding");
    frame(
        &FrameHeader::new(
            MessageType::Hello,
            SESSION_ID,
            0,
            u32::try_from(HELLO_PAYLOAD_BYTES).expect("a small payload"),
        ),
        &payload,
    )
}

fn empty_frame(message_type: MessageType, request_id: u64) -> Vec<u8> {
    frame(
        &FrameHeader::new(message_type, SESSION_ID, request_id, 0),
        &[],
    )
}

fn write_frame(worker: &mut IsolatedWorker, bytes: &[u8]) {
    worker
        .write_all(bytes, deadline(Duration::from_secs(5)), &none())
        .expect("the worker accepts a frame");
}

/// Reads one complete frame and returns its header plus payload bytes.
fn read_frame(worker: &mut IsolatedWorker) -> (FrameHeader, Vec<u8>) {
    let mut header_bytes = [0_u8; FRAME_HEADER_BYTES];
    worker
        .read_exact(&mut header_bytes, deadline(Duration::from_secs(5)), &none())
        .expect("the worker sends a frame header");
    let header = decode_frame_header(&header_bytes).expect("a canonical frame header");
    assert_eq!(header.session_id, SESSION_ID);
    let mut payload = vec![0_u8; header.payload_length as usize];
    if !payload.is_empty() {
        worker
            .read_exact(&mut payload, deadline(Duration::from_secs(5)), &none())
            .expect("the worker sends the declared payload");
    }
    (header, payload)
}

/// Drives the handshake and returns the worker with `ready` consumed.
fn handshake() -> IsolatedWorker {
    let mut worker = launch();
    write_frame(&mut worker, &hello_frame());
    let (header, payload) = read_frame(&mut worker);
    assert_eq!(header.message_type, MessageType::Ready);
    assert_eq!(header.request_id, 0);
    assert!(payload.is_empty());
    decode_ready_payload(&FrameView::new(header, &payload)).expect("a canonical ready payload");
    worker
}

/// The channel must be gone with no reply. A worker that rejects a header
/// without consuming the payload it declared closes the socket with bytes
/// still queued, which the kernel answers with a reset rather than a clean
/// end-of-file, so both outcomes count.
fn assert_channel_gone(worker: &mut IsolatedWorker, message: &str) {
    let mut byte = [0_u8; 1];
    let outcome = worker.read_exact(&mut byte, deadline(Duration::from_secs(5)), &none());
    assert!(
        matches!(
            outcome,
            Err(IsolatedWorkerError::PeerClosed | IsolatedWorkerError::IoFailure)
        ),
        "{message}, got {outcome:?}"
    );
}

fn assert_exit(worker: &mut IsolatedWorker, expected: IsolatedWorkerExitKind, message: &str) {
    assert_eq!(
        worker
            .wait(deadline(Duration::from_secs(10)))
            .expect("the worker is reaped"),
        expected,
        "{message}"
    );
}

// ------------------------------------------------------------ scenarios ----

#[test]
#[ignore = "runs only in the staged install layout"]
fn the_worker_completes_the_handshake_and_shuts_down_cleanly() {
    if !staged() {
        return;
    }
    let mut worker = handshake();
    write_frame(&mut worker, &empty_frame(MessageType::Shutdown, 0));
    assert_channel_gone(&mut worker, "shutdown must end the channel");
    assert_exit(
        &mut worker,
        IsolatedWorkerExitKind::Clean,
        "an orderly shutdown must exit cleanly",
    );
}

#[test]
#[ignore = "runs only in the staged install layout"]
fn an_enumerate_request_is_refused_by_the_compile_fixed_dispatcher() {
    if !staged() {
        return;
    }
    let mut worker = handshake();
    write_frame(&mut worker, &empty_frame(MessageType::Enumerate, 1));
    // `unsupported` is terminal and emits no frame at all, so the channel goes
    // straight to end-of-file after `ready`.
    assert_channel_gone(&mut worker, "a refused request must not be answered");
    assert_exit(
        &mut worker,
        IsolatedWorkerExitKind::Failed,
        "a refused request must not look like a clean shutdown",
    );
}

#[test]
#[ignore = "runs only in the staged install layout"]
fn a_stream_request_is_refused_by_the_compile_fixed_dispatcher() {
    if !staged() {
        return;
    }
    let mut worker = handshake();
    let mut payload = [0_u8; STREAM_ENTRY_PAYLOAD_BYTES];
    encode_stream_entry_payload(&StreamEntry { source_token: 7 }, &mut payload)
        .expect("stream request encoding");
    write_frame(
        &mut worker,
        &frame(
            &FrameHeader::new(
                MessageType::StreamEntry,
                SESSION_ID,
                1,
                u32::try_from(STREAM_ENTRY_PAYLOAD_BYTES).expect("a small payload"),
            ),
            &payload,
        ),
    );
    assert_channel_gone(&mut worker, "a refused stream must not be answered");
    assert_exit(
        &mut worker,
        IsolatedWorkerExitKind::Failed,
        "a refused stream must not look like a clean shutdown",
    );
}

#[test]
#[ignore = "runs only in the staged install layout"]
fn a_malformed_first_frame_is_refused_without_a_reply() {
    if !staged() {
        return;
    }
    let mut worker = launch();
    let mut bytes = hello_frame();
    bytes[0] = 0; // break the magic
    write_frame(&mut worker, &bytes);
    assert_channel_gone(&mut worker, "a malformed hello must not be answered");
    assert_exit(
        &mut worker,
        IsolatedWorkerExitKind::Failed,
        "a malformed hello must fail closed",
    );
}

#[test]
#[ignore = "runs only in the staged install layout"]
fn an_immediate_peer_close_is_an_orderly_end_of_the_worker() {
    if !staged() {
        return;
    }
    let mut worker = launch();
    worker.close_channel();
    assert_exit(
        &mut worker,
        IsolatedWorkerExitKind::Clean,
        "a parent that closes instead of sending shutdown is not a worker failure",
    );
}
