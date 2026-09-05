//! The two capabilities the service borrows, and the scratch buffers it
//! borrows with them.
//!
//! Both traits are *trusted implementation* interfaces: the service never
//! treats a transport or a dispatcher as untrusted input, but it does
//! validate every value they produce before it reaches the wire, exactly as
//! the C++ contract did.

use ohl_parser_protocol::{EntryBatchEntry, ReadReply, SourceReadPolicy};

/// The outcome of one exact read or write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IoStatus {
    /// The complete span was transferred.
    Ok,
    /// The peer closed the channel in an orderly way.
    PeerClosed,
    /// The transfer failed.
    Failed,
}

/// The outcome of a non-consuming input probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InputStatus {
    /// No frame bytes are waiting; the service may run a dispatch step.
    Unavailable,
    /// At least one byte is waiting; the service must read a frame.
    Available,
    /// The peer closed the channel.
    PeerClosed,
    /// The probe failed.
    Failed,
}

/// Synchronous, exact-I/O access to the already-open worker channel.
///
/// A successful [`Transport::read_exact`] or [`Transport::write_all`]
/// transfers the complete non-empty span; a partial transfer must be reported
/// as a failure, never as success. [`Transport::probe_input`] must not
/// consume bytes. [`Transport::abort_io`] and [`Transport::close_io`] are
/// idempotent. No method may re-enter the service.
///
/// The C++ table required the implementation not to retain the spans it was
/// handed; here the lifetimes say so.
pub trait Transport {
    /// Fills `destination` completely.
    fn read_exact(&mut self, destination: &mut [u8]) -> IoStatus;

    /// Writes `source` completely.
    fn write_all(&mut self, source: &[u8]) -> IoStatus;

    /// Reports whether input is waiting, without consuming it.
    fn probe_input(&mut self) -> InputStatus;

    /// Tears the channel down after a failure. Idempotent.
    fn abort_io(&mut self);

    /// Closes the channel after a canonical shutdown. Idempotent.
    fn close_io(&mut self);
}

/// The top-level request kind a dispatcher was asked to serve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Operation {
    /// A bounded enumeration of the pinned source.
    Enumerate,
    /// A bounded single-entry stream out of the pinned source.
    Stream,
}

/// Why a dispatcher refused to make progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DispatchError {
    /// This worker cannot serve the request at all. Terminal, and no frame
    /// is put on the wire for it.
    Unsupported,
    /// The dispatcher failed. Terminal.
    Failed,
}

/// Exactly one bounded action produced by [`Dispatcher::step`].
///
/// Every borrow is tied to the dispatcher, so the service must encode the
/// action before it may call the dispatcher again, and it can never read a
/// view the dispatcher has since invalidated.
#[derive(Debug)]
pub enum DispatchAction<'dispatch> {
    /// Ask the parent to read `length` bytes at `offset` from the pinned
    /// source. Only one read may be outstanding.
    NeedRead {
        /// The offset in the pinned source.
        offset: u64,
        /// The number of bytes wanted.
        length: u32,
    },
    /// Emit a bounded batch of enumerated entries (enumeration only).
    EntryBatch(&'dispatch [EntryBatchEntry<'dispatch>]),
    /// Emit a bounded chunk of the streamed entry (stream only).
    DataChunk(&'dispatch [u8]),
    /// The request finished canonically.
    Complete,
}

/// The trusted parser back end.
///
/// It owns no transport and no source capability: it can only ask the service
/// to relay bounded actions. Every method is synchronous, allocation-free and
/// must perform bounded work; none may re-enter the service.
pub trait Dispatcher {
    /// Starts one top-level request.
    ///
    /// Returns the exact number of data bytes a stream will emit before
    /// canonical completion; an enumeration must return `0`.
    ///
    /// # Errors
    /// [`DispatchError::Unsupported`] when this worker cannot serve the
    /// request at all, [`DispatchError::Failed`] otherwise.
    fn begin(
        &mut self,
        operation: Operation,
        source_token: u64,
        source_policy: &SourceReadPolicy,
    ) -> Result<u64, DispatchError>;

    /// Performs one bounded unit of work and returns exactly one action.
    ///
    /// # Errors
    /// [`DispatchError::Unsupported`] or [`DispatchError::Failed`].
    fn step(&mut self) -> Result<DispatchAction<'_>, DispatchError>;

    /// Consumes the answer to the single outstanding read request.
    ///
    /// # Errors
    /// [`DispatchError::Unsupported`] or [`DispatchError::Failed`].
    fn accept_read_reply(&mut self, reply: &ReadReply<'_>) -> Result<(), DispatchError>;

    /// Abandons the active request. Called at most once per request.
    fn cancel(&mut self);

    /// Retires a canonically completed request. Called at most once per
    /// request, and never together with [`Dispatcher::cancel`].
    fn end(&mut self);
}

/// A compile-fixed dispatcher that refuses every request.
///
/// This is the dispatcher the shipped worker binary hosts today: the Rust
/// import path has no production parser yet, so every `enumerate` and
/// `stream_entry` is answered with [`DispatchError::Unsupported`], which is
/// terminal and puts no frame on the wire. It exists so the whole worker
/// lifetime - handshake, sandbox, teardown, exit status - can be exercised
/// end to end before a parser exists.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct UnsupportedDispatcher;

impl UnsupportedDispatcher {
    /// Builds the dispatcher.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Dispatcher for UnsupportedDispatcher {
    fn begin(
        &mut self,
        _operation: Operation,
        _source_token: u64,
        _source_policy: &SourceReadPolicy,
    ) -> Result<u64, DispatchError> {
        Err(DispatchError::Unsupported)
    }

    fn step(&mut self) -> Result<DispatchAction<'_>, DispatchError> {
        Err(DispatchError::Unsupported)
    }

    fn accept_read_reply(&mut self, _reply: &ReadReply<'_>) -> Result<(), DispatchError> {
        Err(DispatchError::Unsupported)
    }

    fn cancel(&mut self) {}

    fn end(&mut self) {}
}

/// The caller-owned scratch storage the service encodes and decodes through.
///
/// Both slices must hold [`ohl_parser_protocol::MAXIMUM_FRAME_PAYLOAD_BYTES`]
/// bytes. They are `&mut`, so the borrow checker already proves they are
/// disjoint from each other and from everything the transport or dispatcher
/// can reach; the C++ overlap checks have no Rust counterpart.
#[derive(Debug)]
pub struct ServiceBuffers<'buffers> {
    /// Payload bytes of the frame currently being received.
    pub receive_payload: &'buffers mut [u8],
    /// Payload bytes of the frame currently being sent.
    pub send_payload: &'buffers mut [u8],
}

// Blanket forwarding so a test or a composition root can keep ownership of a
// transport or dispatcher and still hand the service a capability.

impl<T: Transport + ?Sized> Transport for &mut T {
    fn read_exact(&mut self, destination: &mut [u8]) -> IoStatus {
        (**self).read_exact(destination)
    }

    fn write_all(&mut self, source: &[u8]) -> IoStatus {
        (**self).write_all(source)
    }

    fn probe_input(&mut self) -> InputStatus {
        (**self).probe_input()
    }

    fn abort_io(&mut self) {
        (**self).abort_io();
    }

    fn close_io(&mut self) {
        (**self).close_io();
    }
}

impl<D: Dispatcher + ?Sized> Dispatcher for &mut D {
    fn begin(
        &mut self,
        operation: Operation,
        source_token: u64,
        source_policy: &SourceReadPolicy,
    ) -> Result<u64, DispatchError> {
        (**self).begin(operation, source_token, source_policy)
    }

    fn step(&mut self) -> Result<DispatchAction<'_>, DispatchError> {
        (**self).step()
    }

    fn accept_read_reply(&mut self, reply: &ReadReply<'_>) -> Result<(), DispatchError> {
        (**self).accept_read_reply(reply)
    }

    fn cancel(&mut self) {
        (**self).cancel();
    }

    fn end(&mut self) {
        (**self).end();
    }
}
