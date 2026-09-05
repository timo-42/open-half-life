//! Parent-side parser import sessions for the Open Half-Life Rust port.
//!
//! This crate is the privileged half of the OWP/1 trust boundary: it drives a
//! sandboxed parser worker over the wire protocol in [`ohl_parser_protocol`],
//! services the worker's bounded source reads from the pinned capability
//! inside an [`ohl_media::ValidatedMedia`] proof, and promotes only fully
//! validated results into a bounded catalog.
//!
//! It is the Rust port of the C++ `media::Parser*` session family:
//!
//! | C++ (`src/media/…`) | Rust module |
//! | --- | --- |
//! | `parser_frame_channel` | [`frame_channel`], [`io`] |
//! | `parser_source_read_broker` | [`source_read_broker`] |
//! | `parser_result_session` + `payload_layout` | [`result_session`], [`catalog`] |
//! | `parser_parent_handshake` | [`handshake`] |
//! | `parser_parent_session` | [`parent_session`] |
//! | `parser_process_session` | [`process_session`] |
//!
//! # What this crate does not do
//!
//! It launches no process (the R4.7 `IsolatedWorker` adapter does), opens no
//! path, stages no file and publishes no payload. Every capability it needs is
//! handed to it: a [`ValidatedMedia`](ohl_media::ValidatedMedia) proof, an
//! [`ExactIo`] channel, and a [`WorkerProcess`] whose lifetime it owns.
//!
//! # Security properties
//!
//! - **Sealed capabilities.** [`ExactIo`], [`SourceOps`] and [`WorkerProcess`]
//!   cannot be implemented outside this crate, so no media-derived code can
//!   become the transport, the source, or the process.
//! - **Typestate lifecycle.** [`ParserSession`] transitions consume the
//!   session, so `receive_one` does not exist while idle and two overlapping
//!   operations are not expressible.
//! - **Move-only proofs.** [`HandshakeProof`] and [`PreparedReply`] are
//!   consumed by their single use, so double consumption is a compile error.
//! - **Borrowed views.** A [`CatalogView`] borrows the session that promoted
//!   it, and a received `FrameView` borrows the [`FrameBuffer`] it was read
//!   into; both C++ "this span is invalidated by …" comments become borrows.
//! - **Sanitized errors.** Every error type is a fixed, payload-free code
//!   whose `Display` cannot interpolate a media byte, a path, or an OS string.
//! - No `unsafe`: the crate inherits the workspace-wide
//!   `unsafe_code = "forbid"`.

#![forbid(unsafe_code)]

pub mod catalog;
pub mod frame_channel;
pub mod handshake;
pub mod io;
pub mod parent_session;
pub mod process_session;
pub mod result_session;
pub mod source_read_broker;
pub mod testing;

pub use catalog::{
    Catalog, CatalogGeneration, CatalogView, EntryMetadata, ImportLimits, LayoutError,
    NormalizedPath, PlannedEntry, SourceToken, WorkerEpoch, plan_catalog,
};
pub use frame_channel::{ChannelError, FrameBuffer, FrameChannel};
pub use handshake::{HandshakeError, HandshakeProof, perform_parent_handshake};
pub use io::{CancellationSource, CancellationToken, ExactIo, IoError};
pub use parent_session::{
    CancelStep, Cancelled, Cancelling, Closed, Enumerating, Idle, ParserSession, RequestStep,
    SessionBuffers, SessionError, SessionPhase, SessionPhaseKind, Streaming, TerminalSession,
    create_parser_session, create_parser_session_with_ops,
};
pub use process_session::{
    AllocatorExhausted, OpenError, OpenFailure, ProcessSession, ProcessState, SessionAllocation,
    SessionConfig, SessionIdAllocator, ShutdownError, ShutdownFailure, ShutdownReady, WaitOutcome,
    WorkerExit, WorkerProcess,
};
pub use result_session::{
    ByteSink, ReadRequestOutcome, ResultSession, ResultSessionError, SinkRejected,
};
pub use source_read_broker::{
    NativeSourceOps, PrepareOutcome, PreparedReply, ReplyTicket, SourceOps, SourceReadBroker,
    SourceReadError, SourceReadLimits,
};
