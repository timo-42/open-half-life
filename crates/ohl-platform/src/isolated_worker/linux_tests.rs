//! End-to-end tests for the Linux x86-64 isolated-worker backend.
//!
//! Every test drives a real confined child built from
//! `crates/ohl-test-worker/image`: a freestanding, statically linked
//! `#![no_std]` program that issues raw syscalls only. That is what makes the
//! seccomp assertions meaningful - a libc-based helper would need dozens of
//! syscalls the policy deliberately denies.

use super::{
    IsolatedWorker, IsolatedWorkerCancellationSource, IsolatedWorkerCancellationToken,
    IsolatedWorkerError, IsolatedWorkerExitKind, IsolatedWorkerService, launch_isolated_worker,
    launch_isolated_worker_from_image,
};

use ohl_test_worker::protocol;
use ohl_test_worker::{TestWorkerVariant, build_test_worker_image};

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// `SIGILL`, raised by the `ud2` in the crash mode.
const SIGILL: i32 = 4;
/// `SIGSYS`, raised by `SECCOMP_RET_KILL_PROCESS`.
const SIGSYS: i32 = 31;

fn image(variant: TestWorkerVariant) -> PathBuf {
    static READY: OnceLock<PathBuf> = OnceLock::new();
    static NEVER_READY: OnceLock<PathBuf> = OnceLock::new();
    let cell = match variant {
        TestWorkerVariant::Ready => &READY,
        TestWorkerVariant::NeverReady => &NEVER_READY,
    };
    cell.get_or_init(|| build_test_worker_image(variant).expect("the test worker image builds"))
        .clone()
}

fn deadline(after: Duration) -> Instant {
    Instant::now().checked_add(after).expect("a near deadline")
}

fn launch_ready() -> IsolatedWorker {
    launch_isolated_worker_from_image(
        &image(TestWorkerVariant::Ready),
        deadline(Duration::from_secs(5)),
    )
    .expect("a confined worker launches")
}

/// Launches and keeps only the failure, so image-verification tests can use
/// `assert_eq!` on a comparable value.
fn launch_failure(path: &Path) -> IsolatedWorkerError {
    match launch_isolated_worker_from_image(path, deadline(Duration::from_secs(5))) {
        Ok(_) => panic!("an unverifiable image must never launch"),
        Err(error) => error,
    }
}

fn none() -> IsolatedWorkerCancellationToken {
    IsolatedWorkerCancellationToken::default()
}

fn send_frame(worker: &mut IsolatedWorker, payload: &[u8]) -> Result<(), IsolatedWorkerError> {
    let length = u32::try_from(payload.len()).expect("a small frame");
    worker.write_all(
        &length.to_le_bytes(),
        deadline(Duration::from_secs(5)),
        &none(),
    )?;
    worker.write_all(payload, deadline(Duration::from_secs(5)), &none())
}

fn receive_frame(worker: &mut IsolatedWorker) -> Result<Vec<u8>, IsolatedWorkerError> {
    let mut length = [0u8; 4];
    worker.read_exact(&mut length, deadline(Duration::from_secs(5)), &none())?;
    let length = u32::from_le_bytes(length) as usize;
    assert!(
        length <= protocol::MAX_FRAME_BYTES,
        "worker framing is bounded"
    );
    let mut payload = vec![0u8; length];
    worker.read_exact(&mut payload, deadline(Duration::from_secs(5)), &none())?;
    Ok(payload)
}

/// Stages `bytes` at a fresh temporary path with `mode`.
fn stage(bytes: &[u8], mode: u32, name: &str) -> (tempfile::TempDir, PathBuf) {
    use std::os::unix::fs::PermissionsExt as _;
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join(name);
    std::fs::write(&path, bytes).expect("staging writes");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).expect("chmod");
    (directory, path)
}

#[test]
fn the_backend_constants_match_the_worker_protocol() {
    assert_eq!(
        super::backend::READY_ATTESTATION,
        protocol::READY_ATTESTATION
    );
}

#[test]
fn a_worker_echoes_a_frame_and_then_exits_cleanly_on_an_orderly_close() {
    let mut worker = launch_ready();
    let mut request = vec![protocol::MODE_ECHO_REVERSED];
    request.extend_from_slice(b"half-life");
    send_frame(&mut worker, &request).expect("the request is written");

    let reply = receive_frame(&mut worker).expect("the reply arrives");
    assert_eq!(reply, b"efil-flah");

    worker.close_channel();
    assert_eq!(
        worker.wait(deadline(Duration::from_secs(5))),
        Ok(IsolatedWorkerExitKind::Clean)
    );
    // The terminal result is cached, so a second wait cannot reap twice.
    assert_eq!(
        worker.wait(deadline(Duration::from_secs(5))),
        Ok(IsolatedWorkerExitKind::Clean)
    );
}

#[test]
fn several_frames_round_trip_on_one_worker() {
    let mut worker = launch_ready();
    for round in 0..8u8 {
        let mut request = vec![protocol::MODE_ECHO_REVERSED];
        request.extend_from_slice(&[round; 5]);
        request.push(0xff);
        send_frame(&mut worker, &request).expect("the request is written");
        let reply = receive_frame(&mut worker).expect("the reply arrives");
        assert_eq!(reply, [&[0xffu8][..], &[round; 5][..]].concat());
    }
    worker.close_channel();
    assert_eq!(
        worker.wait(deadline(Duration::from_secs(5))),
        Ok(IsolatedWorkerExitKind::Clean)
    );
}

#[test]
fn a_hanging_worker_survives_an_orderly_close_and_is_then_terminated() {
    let mut worker = launch_ready();
    send_frame(&mut worker, &[protocol::MODE_HANG]).expect("the mode is selected");

    worker.close_channel();
    assert_eq!(
        worker.wait(deadline(Duration::from_millis(300))),
        Err(IsolatedWorkerError::Timeout),
        "a hung worker must not be mistaken for a finished one"
    );
    assert_eq!(
        worker.terminate_and_wait(deadline(Duration::from_secs(5))),
        Ok(IsolatedWorkerExitKind::Terminated)
    );
}

#[test]
fn a_crashing_worker_is_reported_as_crashed() {
    let mut worker = launch_ready();
    send_frame(&mut worker, &[protocol::MODE_CRASH]).expect("the mode is selected");
    assert_eq!(
        worker.wait(deadline(Duration::from_secs(5))),
        Ok(IsolatedWorkerExitKind::Crashed)
    );
    assert_eq!(worker.terminating_signal(), Some(SIGILL));
}

#[test]
fn a_forbidden_syscall_is_killed_by_seccomp() {
    let mut worker = launch_ready();
    send_frame(&mut worker, &[protocol::MODE_FORBIDDEN_SYSCALL]).expect("the mode is selected");
    assert_eq!(
        worker.wait(deadline(Duration::from_secs(5))),
        Ok(IsolatedWorkerExitKind::Crashed)
    );
    assert_eq!(
        worker.terminating_signal(),
        Some(SIGSYS),
        "openat(2) must be denied by SECCOMP_RET_KILL_PROCESS, not by anything else"
    );
}

#[test]
fn a_non_zero_exit_status_is_reported_as_failed() {
    let mut worker = launch_ready();
    send_frame(&mut worker, &[protocol::MODE_EXIT, 7]).expect("the mode is selected");
    assert_eq!(
        worker.wait(deadline(Duration::from_secs(5))),
        Ok(IsolatedWorkerExitKind::Failed)
    );
}

#[test]
fn a_worker_that_never_attests_readiness_times_out() {
    let started = Instant::now();
    let outcome = launch_isolated_worker_from_image(
        &image(TestWorkerVariant::NeverReady),
        deadline(Duration::from_millis(400)),
    )
    .err();
    assert_eq!(outcome, Some(IsolatedWorkerError::Timeout));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the startup deadline, not the ceiling, must bound the wait"
    );
}

#[test]
fn cancellation_wakes_a_blocked_read() {
    let mut worker = launch_ready();
    let source = IsolatedWorkerCancellationSource::new();
    let token = source.token();

    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        source.request_cancellation();
    });

    let mut destination = [0u8; 4];
    let outcome = worker.read_exact(&mut destination, deadline(Duration::from_secs(20)), &token);
    canceller.join().expect("the cancelling thread finishes");
    assert_eq!(outcome, Err(IsolatedWorkerError::Cancelled));

    // Cancellation poisons the channel permanently.
    assert_eq!(
        worker.read_exact(&mut destination, deadline(Duration::from_secs(1)), &none()),
        Err(IsolatedWorkerError::InvalidState)
    );
}

#[test]
fn an_expired_deadline_reports_a_timeout_rather_than_blocking() {
    let mut worker = launch_ready();
    let mut destination = [0u8; 1];
    assert_eq!(
        worker.read_exact(&mut destination, Instant::now(), &none()),
        Err(IsolatedWorkerError::Timeout)
    );
}

#[test]
fn zero_length_and_oversized_transfers_are_rejected() {
    let mut worker = launch_ready();
    assert_eq!(
        worker.write_all(&[], deadline(Duration::from_secs(1)), &none()),
        Err(IsolatedWorkerError::InvalidArgument)
    );
    let oversized = vec![0u8; super::MAX_ISOLATED_WORKER_IO_BYTES + 1];
    assert_eq!(
        worker.write_all(&oversized, deadline(Duration::from_secs(1)), &none()),
        Err(IsolatedWorkerError::TransferTooLarge)
    );
}

#[test]
fn a_closed_channel_rejects_further_traffic() {
    let mut worker = launch_ready();
    worker.close_channel();
    assert_eq!(
        worker.write_all(b"x", deadline(Duration::from_secs(1)), &none()),
        Err(IsolatedWorkerError::InvalidState)
    );
}

#[test]
fn dropping_a_live_worker_reaps_it() {
    let mut worker = launch_ready();
    send_frame(&mut worker, &[protocol::MODE_HANG]).expect("the mode is selected");
    let started = Instant::now();
    drop(worker);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "Drop must terminate and reap rather than wait out the child"
    );
}

#[test]
fn twenty_consecutive_launches_all_succeed() {
    for round in 0..20u8 {
        let mut worker = launch_ready();
        let request = vec![protocol::MODE_ECHO_REVERSED, round, round.wrapping_add(1)];
        send_frame(&mut worker, &request).expect("the request is written");
        assert_eq!(
            receive_frame(&mut worker).expect("the reply arrives"),
            vec![round.wrapping_add(1), round]
        );
        worker.close_channel();
        assert_eq!(
            worker.wait(deadline(Duration::from_secs(5))),
            Ok(IsolatedWorkerExitKind::Clean),
            "round {round}"
        );
    }
}

// --- image verification ----------------------------------------------------

#[test]
fn a_writable_image_is_rejected() {
    let bytes = std::fs::read(image(TestWorkerVariant::Ready)).expect("the image is readable");
    let (_directory, path) = stage(&bytes, 0o755, "ohl-media-parser-worker");
    assert_eq!(
        launch_failure(&path),
        IsolatedWorkerError::ServiceIdentityMismatch
    );
}

#[test]
fn a_non_executable_image_is_rejected() {
    let bytes = std::fs::read(image(TestWorkerVariant::Ready)).expect("the image is readable");
    let (_directory, path) = stage(&bytes, 0o444, "ohl-media-parser-worker");
    assert_eq!(
        launch_failure(&path),
        IsolatedWorkerError::ServiceIdentityMismatch
    );
}

#[test]
fn a_symlinked_image_is_rejected() {
    let real = image(TestWorkerVariant::Ready);
    let directory = tempfile::tempdir().expect("a temporary directory");
    let link = directory.path().join("ohl-media-parser-worker");
    std::os::unix::fs::symlink(&real, &link).expect("the symlink is created");
    assert_eq!(
        launch_failure(&link),
        IsolatedWorkerError::ServiceIdentityMismatch,
        "O_NOFOLLOW must refuse to traverse the final component"
    );
}

#[test]
fn a_dynamically_linked_image_is_rejected() {
    let dynamic = std::env::current_exe().expect("the test binary has a path");
    let bytes = std::fs::read(dynamic).expect("the test binary is readable");
    let (_directory, path) = stage(&bytes, 0o555, "ohl-media-parser-worker");
    assert_eq!(
        launch_failure(&path),
        IsolatedWorkerError::ServiceIdentityMismatch,
        "an interpreted or dynamic ELF is not a confinable image"
    );
}

#[test]
fn a_non_elf_image_is_rejected() {
    let (_directory, path) = stage(b"#!/bin/sh\nexit 0\n", 0o555, "ohl-media-parser-worker");
    assert_eq!(
        launch_failure(&path),
        IsolatedWorkerError::ServiceIdentityMismatch
    );
}

#[test]
fn a_missing_image_is_reported_as_an_unavailable_service() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("absent");
    assert_eq!(
        launch_failure(&path),
        IsolatedWorkerError::ServiceUnavailable
    );
}

#[test]
fn the_compile_fixed_install_location_is_the_only_production_source() {
    // A test binary never has `libexec/open-half-life/` beside it, so the
    // production resolver must fail rather than fall back to anything else.
    let outcome = launch_isolated_worker(
        IsolatedWorkerService::MediaParser,
        deadline(Duration::from_secs(5)),
    )
    .err();
    assert!(
        matches!(
            outcome,
            Some(
                IsolatedWorkerError::ServiceUnavailable
                    | IsolatedWorkerError::ServiceIdentityMismatch
            )
        ),
        "unexpected production-resolution outcome: {outcome:?}"
    );
}

#[test]
fn the_staged_image_lives_where_the_builder_says_it_does() {
    let path = image(TestWorkerVariant::Ready);
    assert!(Path::new(&path).is_file());
}
