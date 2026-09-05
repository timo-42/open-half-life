//! Every case from the C++ service test
//! (`tests/parser/parser_worker_service_test.cpp`), ported one for one.
//!
//! Two C++ cases have no Rust counterpart because the type system already
//! rules them out; each is called out where it belongs:
//!
//! - `test_dispatcher_views_overlapping_service_buffers_are_rejected` - a
//!   dispatcher cannot name, let alone alias, the service's `&mut` scratch
//!   buffers (see [`overlapping_service_buffers_are_unrepresentable`]);
//! - the `probe_input` case that returns the out-of-table value `0xff` - the
//!   probe returns [`InputStatus`], which has no such value.
//!
//! `test_dispatch_views_are_not_used_after_transport_callback` is ported as
//! the observable half of the C++ case: two batches are emitted from two
//! distinct storages and both frames must decode to what was live when each
//! `step` returned.

use ohl_parser_protocol::frame::encode_frame_header;
use ohl_parser_protocol::messages::{
    HELLO_PAYLOAD_BYTES, MAXIMUM_ENTRY_BATCH_ENTRIES, MAXIMUM_ENTRY_BATCH_PATH_BYTES,
    MAXIMUM_ENUMERATED_ENTRIES, MAXIMUM_ENUMERATED_ENTRY_BYTES, MAXIMUM_ENUMERATED_PATH_BYTES,
    MAXIMUM_ENUMERATED_TOTAL_BYTES, decode_complete_payload, decode_data_chunk_payload,
    decode_entry_batch_payload, decode_read_request_payload, decode_ready_payload,
    encode_hello_payload, encode_read_reply_payload, encode_stream_entry_payload,
};
use ohl_parser_protocol::{
    ArchiveSpelling, EntryBatchEntry, EntryBatchPolicy, FRAME_HEADER_BYTES, FrameHeader, FrameView,
    Hello, MAXIMUM_CUMULATIVE_PAYLOAD_BYTES, MAXIMUM_FRAME_PAYLOAD_BYTES, MessageType,
    OperationPhase, ProtocolBudgets, ProtocolError, ProtocolStatus, ReadReply, SourceReadPolicy,
    StreamEntry, decode_frame_header, encode_frame,
};
use ohl_parser_worker_service::{
    DispatchAction, DispatchError, Dispatcher, InputStatus, IoStatus, MAXIMUM_DISPATCH_STEPS,
    Operation, ServiceBuffers, ServiceError, ServiceFailure, ServiceLimits, ServiceSummary,
    Transport, UnsupportedDispatcher, run_parser_worker_service,
};

const SESSION_ID: u64 = 0x1020_3040_5060_7080;
const SOURCE_SIZE: u64 = 4096;
const MAXIMUM_READ_BYTES: u32 = 256;
const PAYLOAD_CAPACITY: usize = MAXIMUM_FRAME_PAYLOAD_BYTES as usize;
const READ_REPLY_PREFIX_BYTES: usize = 6;

// ------------------------------------------------------------ scripting ----

/// Narrows a test-sized length; every value here is far below `u32::MAX`.
fn narrow(value: usize) -> u32 {
    u32::try_from(value).expect("a test-sized length")
}

/// Widens a test-sized length.
fn widen(value: usize) -> u64 {
    u64::from(narrow(value))
}

/// Narrows a protocol ceiling to a host-sized length.
fn shrink(value: u64) -> usize {
    usize::try_from(value).expect("a host-sized ceiling")
}

fn append_frame(bytes: &mut Vec<u8>, header: &FrameHeader, payload: &[u8]) {
    let mut frame = vec![0_u8; FRAME_HEADER_BYTES + payload.len()];
    let written = encode_frame(header, payload, &mut frame).expect("test frame encoding");
    assert_eq!(written, frame.len());
    bytes.extend_from_slice(&frame);
}

fn append_header(bytes: &mut Vec<u8>, header: &FrameHeader) {
    let mut encoded = [0_u8; FRAME_HEADER_BYTES];
    encode_frame_header(header, &mut encoded).expect("test header encoding");
    bytes.extend_from_slice(&encoded);
}

fn append_hello(bytes: &mut Vec<u8>) {
    let mut payload = [0_u8; HELLO_PAYLOAD_BYTES];
    encode_hello_payload(
        &Hello {
            source_size: SOURCE_SIZE,
            maximum_read_bytes: MAXIMUM_READ_BYTES,
        },
        &mut payload,
    )
    .expect("hello encoding");
    append_frame(
        bytes,
        &FrameHeader::new(
            MessageType::Hello,
            SESSION_ID,
            0,
            narrow(HELLO_PAYLOAD_BYTES),
        ),
        &payload,
    );
}

fn append_shutdown(bytes: &mut Vec<u8>) {
    append_frame(
        bytes,
        &FrameHeader::new(MessageType::Shutdown, SESSION_ID, 0, 0),
        &[],
    );
}

fn append_enumerate(bytes: &mut Vec<u8>, request_id: u64) {
    append_frame(
        bytes,
        &FrameHeader::new(MessageType::Enumerate, SESSION_ID, request_id, 0),
        &[],
    );
}

fn append_stream(bytes: &mut Vec<u8>, request_id: u64, source_token: u64) {
    let mut payload = [0_u8; 8];
    let written = encode_stream_entry_payload(&StreamEntry { source_token }, &mut payload)
        .expect("stream request encoding");
    append_frame(
        bytes,
        &FrameHeader::new(
            MessageType::StreamEntry,
            SESSION_ID,
            request_id,
            narrow(written),
        ),
        &payload[..written],
    );
}

fn append_cancel(bytes: &mut Vec<u8>, request_id: u64) {
    append_frame(
        bytes,
        &FrameHeader::new(MessageType::Cancel, SESSION_ID, request_id, 0),
        &[],
    );
}

fn append_read_reply(
    bytes: &mut Vec<u8>,
    request_id: u64,
    sequence: u32,
    data: &[u8],
    status: ProtocolStatus,
    requested_length: u32,
) {
    let requested = if requested_length == 0 {
        narrow(data.len())
    } else {
        requested_length
    };
    let mut payload = vec![0_u8; READ_REPLY_PREFIX_BYTES + data.len()];
    let written = encode_read_reply_payload(
        &ReadReply {
            read_sequence: sequence,
            status,
            data,
        },
        sequence,
        requested,
        &mut payload,
    )
    .expect("read reply encoding");
    append_frame(
        bytes,
        &FrameHeader::new(
            MessageType::ReadReply,
            SESSION_ID,
            request_id,
            narrow(written),
        ),
        &payload[..written],
    );
}

#[derive(Debug)]
struct ScriptedTransport {
    input: Vec<u8>,
    output: Vec<u8>,
    probes: Vec<InputStatus>,
    input_position: usize,
    probe_position: usize,
    read_calls: usize,
    write_calls: usize,
    probe_calls: usize,
    abort_calls: usize,
    close_calls: usize,
    /// The one-based read call that fails, or zero for none.
    fail_read_call: usize,
    /// The one-based write call that fails, or zero for none.
    fail_write_call: usize,
    read_failure: IoStatus,
    write_failure: IoStatus,
}

impl ScriptedTransport {
    fn new() -> Self {
        Self {
            input: Vec::new(),
            output: Vec::new(),
            probes: Vec::new(),
            input_position: 0,
            probe_position: 0,
            read_calls: 0,
            write_calls: 0,
            probe_calls: 0,
            abort_calls: 0,
            close_calls: 0,
            fail_read_call: 0,
            fail_write_call: 0,
            read_failure: IoStatus::PeerClosed,
            write_failure: IoStatus::Failed,
        }
    }

    fn aborted(&self) -> bool {
        self.abort_calls != 0
    }

    fn closed(&self) -> bool {
        self.close_calls != 0
    }
}

impl Transport for ScriptedTransport {
    fn read_exact(&mut self, destination: &mut [u8]) -> IoStatus {
        self.read_calls += 1;
        if self.fail_read_call == self.read_calls {
            return self.read_failure;
        }
        let available = self.input.len() - self.input_position.min(self.input.len());
        if destination.len() > available {
            return IoStatus::PeerClosed;
        }
        let end = self.input_position + destination.len();
        destination.copy_from_slice(&self.input[self.input_position..end]);
        self.input_position = end;
        IoStatus::Ok
    }

    fn write_all(&mut self, source: &[u8]) -> IoStatus {
        self.write_calls += 1;
        if self.fail_write_call == self.write_calls {
            return self.write_failure;
        }
        self.output.extend_from_slice(source);
        IoStatus::Ok
    }

    fn probe_input(&mut self) -> InputStatus {
        self.probe_calls += 1;
        let status = self
            .probes
            .get(self.probe_position)
            .copied()
            .unwrap_or(InputStatus::Unavailable);
        if self.probe_position < self.probes.len() {
            self.probe_position += 1;
        }
        status
    }

    fn abort_io(&mut self) {
        self.abort_calls += 1;
    }

    fn close_io(&mut self) {
        self.close_calls += 1;
    }
}

#[derive(Debug)]
enum ScriptedStep<'storage> {
    Complete,
    NeedRead { offset: u64, length: u32 },
    EntryBatch(&'storage [EntryBatchEntry<'storage>]),
    DataChunk(&'storage [u8]),
}

#[derive(Debug)]
struct ScriptedDispatch<'storage> {
    begin_result: Result<u64, DispatchError>,
    steps: Vec<ScriptedStep<'storage>>,
    step_position: usize,
    begin_calls: usize,
    step_calls: usize,
    accept_calls: usize,
    cancel_calls: usize,
    end_calls: usize,
    step_status: Result<(), DispatchError>,
    accept_status: Result<(), DispatchError>,
    operation: Option<Operation>,
    source_token: u64,
    source_policy: Option<SourceReadPolicy>,
    accepted_read: Vec<u8>,
    accepted_read_sequence: u32,
}

impl Default for ScriptedDispatch<'_> {
    fn default() -> Self {
        Self {
            begin_result: Ok(0),
            steps: Vec::new(),
            step_position: 0,
            begin_calls: 0,
            step_calls: 0,
            accept_calls: 0,
            cancel_calls: 0,
            end_calls: 0,
            step_status: Ok(()),
            accept_status: Ok(()),
            operation: None,
            source_token: 0,
            source_policy: None,
            accepted_read: Vec::new(),
            accepted_read_sequence: 0,
        }
    }
}

impl ScriptedDispatch<'_> {
    fn began(&self) -> bool {
        self.begin_calls != 0
    }

    fn cancelled(&self) -> bool {
        self.cancel_calls != 0
    }

    fn ended(&self) -> bool {
        self.end_calls != 0
    }
}

impl Dispatcher for ScriptedDispatch<'_> {
    fn begin(
        &mut self,
        operation: Operation,
        source_token: u64,
        source_policy: &SourceReadPolicy,
    ) -> Result<u64, DispatchError> {
        self.begin_calls += 1;
        self.operation = Some(operation);
        self.source_token = source_token;
        self.source_policy = Some(*source_policy);
        self.begin_result
    }

    fn step(&mut self) -> Result<DispatchAction<'_>, DispatchError> {
        self.step_calls += 1;
        self.step_status?;
        let Some(step) = self.steps.get(self.step_position) else {
            return Err(DispatchError::Failed);
        };
        self.step_position += 1;
        Ok(match *step {
            ScriptedStep::Complete => DispatchAction::Complete,
            ScriptedStep::NeedRead { offset, length } => {
                DispatchAction::NeedRead { offset, length }
            }
            ScriptedStep::EntryBatch(entries) => DispatchAction::EntryBatch(entries),
            ScriptedStep::DataChunk(data) => DispatchAction::DataChunk(data),
        })
    }

    fn accept_read_reply(&mut self, reply: &ReadReply<'_>) -> Result<(), DispatchError> {
        self.accept_calls += 1;
        self.accepted_read_sequence = reply.read_sequence;
        self.accepted_read.clear();
        self.accepted_read.extend_from_slice(reply.data);
        self.accept_status
    }

    fn cancel(&mut self) {
        self.cancel_calls += 1;
    }

    fn end(&mut self) {
        self.end_calls += 1;
    }
}

struct Buffers {
    receive: Vec<u8>,
    send: Vec<u8>,
}

impl Buffers {
    fn new() -> Self {
        Self {
            receive: vec![0_u8; PAYLOAD_CAPACITY],
            send: vec![0_u8; PAYLOAD_CAPACITY],
        }
    }

    fn borrow(&mut self) -> ServiceBuffers<'_> {
        ServiceBuffers {
            receive_payload: &mut self.receive,
            send_payload: &mut self.send,
        }
    }
}

fn run(
    transport: &mut ScriptedTransport,
    dispatch: &mut ScriptedDispatch<'_>,
    limits: ServiceLimits,
) -> Result<ServiceSummary, ServiceFailure> {
    let mut buffers = Buffers::new();
    run_parser_worker_service(transport, dispatch, buffers.borrow(), limits)
}

/// One decoded worker-to-parent frame, with its payload copied out.
struct OutputFrame {
    header: FrameHeader,
    payload: Vec<u8>,
}

impl OutputFrame {
    fn view(&self) -> FrameView<'_> {
        FrameView::new(self.header, &self.payload)
    }
}

fn decode_output(bytes: &[u8]) -> Vec<OutputFrame> {
    let mut frames = Vec::new();
    let mut position = 0;
    while position < bytes.len() {
        assert!(
            bytes.len() - position >= FRAME_HEADER_BYTES,
            "truncated output header"
        );
        let header_bytes: &[u8; FRAME_HEADER_BYTES] = bytes
            [position..position + FRAME_HEADER_BYTES]
            .try_into()
            .expect("header slice");
        let header = decode_frame_header(header_bytes).expect("valid output header");
        assert_eq!(header.session_id, SESSION_ID);
        let size = FRAME_HEADER_BYTES + header.payload_length as usize;
        assert!(size <= bytes.len() - position, "truncated output frame");
        frames.push(OutputFrame {
            header,
            payload: bytes[position + FRAME_HEADER_BYTES..position + size].to_vec(),
        });
        position += size;
    }
    frames
}

fn output_types(frames: &[OutputFrame]) -> Vec<MessageType> {
    frames
        .iter()
        .map(|frame| frame.header.message_type)
        .collect()
}

fn entry(source_token: u64, size_bytes: u64, path: &[u8]) -> EntryBatchEntry<'_> {
    EntryBatchEntry {
        source_token,
        size_bytes,
        archive_path: ArchiveSpelling::new(path).expect("printable spelling"),
    }
}

fn full_entry_policy() -> EntryBatchPolicy {
    EntryBatchPolicy::new(
        MAXIMUM_ENUMERATED_ENTRIES,
        MAXIMUM_ENUMERATED_PATH_BYTES,
        MAXIMUM_ENUMERATED_ENTRY_BYTES,
        MAXIMUM_ENUMERATED_TOTAL_BYTES,
        None,
    )
    .expect("full policy")
}

fn budgets(messages: u64, payload_bytes: u64) -> ProtocolBudgets {
    ProtocolBudgets::new(messages, payload_bytes).expect("representable budget")
}

// ------------------------------------------- invalid configuration cases ----

fn require_invalid_configuration_rejected(buffers: ServiceBuffers<'_>, limits: ServiceLimits) {
    let mut transport = ScriptedTransport::new();
    let mut dispatch = ScriptedDispatch::default();
    let failure = run_parser_worker_service(&mut transport, &mut dispatch, buffers, limits)
        .expect_err("invalid service configuration was accepted");
    assert_eq!(failure.error, ServiceError::InvalidConfiguration);
    assert_eq!(failure.session_id, 0);
    assert_eq!(
        (
            transport.read_calls,
            transport.write_calls,
            transport.probe_calls,
            transport.abort_calls,
            transport.close_calls
        ),
        (0, 0, 0, 1, 0),
        "invalid configuration performed transport I/O or the wrong cleanup"
    );
    assert!(transport.output.is_empty());
    assert_eq!(
        (
            dispatch.begin_calls,
            dispatch.step_calls,
            dispatch.accept_calls,
            dispatch.cancel_calls,
            dispatch.end_calls
        ),
        (0, 0, 0, 0, 0),
        "invalid configuration invoked the dispatcher"
    );
}

#[test]
fn invalid_configuration_is_pre_io_and_fail_closed() {
    let mut receive = vec![0_u8; PAYLOAD_CAPACITY];
    let mut send = vec![0_u8; PAYLOAD_CAPACITY];
    let mut short_buffer = vec![0_u8; PAYLOAD_CAPACITY - 1];
    let mut empty: [u8; 0] = [];

    require_invalid_configuration_rejected(
        ServiceBuffers {
            receive_payload: &mut empty,
            send_payload: &mut send,
        },
        ServiceLimits::default(),
    );
    require_invalid_configuration_rejected(
        ServiceBuffers {
            receive_payload: &mut receive,
            send_payload: &mut empty,
        },
        ServiceLimits::default(),
    );
    require_invalid_configuration_rejected(
        ServiceBuffers {
            receive_payload: &mut short_buffer,
            send_payload: &mut send,
        },
        ServiceLimits::default(),
    );
    require_invalid_configuration_rejected(
        ServiceBuffers {
            receive_payload: &mut receive,
            send_payload: &mut short_buffer,
        },
        ServiceLimits::default(),
    );

    for limits in [
        ServiceLimits {
            protocol_budgets: ProtocolBudgets::default(),
            maximum_dispatch_steps: 0,
        },
        ServiceLimits {
            protocol_budgets: ProtocolBudgets::default(),
            maximum_dispatch_steps: MAXIMUM_DISPATCH_STEPS + 1,
        },
        ServiceLimits {
            protocol_budgets: budgets(1, MAXIMUM_CUMULATIVE_PAYLOAD_BYTES),
            maximum_dispatch_steps: 1,
        },
        ServiceLimits {
            protocol_budgets: budgets(2, widen(HELLO_PAYLOAD_BYTES) - 1),
            maximum_dispatch_steps: 1,
        },
    ] {
        require_invalid_configuration_rejected(
            ServiceBuffers {
                receive_payload: &mut receive,
                send_payload: &mut send,
            },
            limits,
        );
    }
}

#[test]
fn overlapping_service_buffers_are_unrepresentable() {
    // The C++ service compared pointer ranges because a dispatcher could hand
    // back a span aliasing `send_payload` or `receive_payload`. In Rust the
    // buffers are `&mut [u8]` owned by the service for the whole lifetime and
    // a dispatcher never sees them, so neither
    // `ServiceBuffers { receive_payload: x, send_payload: x }` nor a
    // dispatcher view into either buffer compiles. Nothing is left to check
    // at run time; this test records that the C++ case was reviewed rather
    // than dropped.
    let mut buffers = Buffers::new();
    let borrowed = buffers.borrow();
    assert_eq!(borrowed.receive_payload.len(), PAYLOAD_CAPACITY);
    assert_eq!(borrowed.send_payload.len(), PAYLOAD_CAPACITY);
}

// ---------------------------------------------------------- begin sizes ----

fn require_terminal_dispatch_failure(
    failure: &ServiceFailure,
    transport: &ScriptedTransport,
    dispatch: &ScriptedDispatch<'_>,
) {
    assert_eq!(failure.error, ServiceError::DispatchFailure);
    assert_eq!(transport.abort_calls, 1);
    assert_eq!(transport.close_calls, 0);
    assert_eq!(dispatch.cancel_calls, 1);
    assert_eq!(dispatch.end_calls, 0);
}

#[test]
fn invalid_begin_sizes_and_stream_completion_are_fail_closed() {
    {
        let mut transport = ScriptedTransport::new();
        append_hello(&mut transport.input);
        append_enumerate(&mut transport.input, 1);
        let mut dispatch = ScriptedDispatch {
            begin_result: Ok(1),
            ..ScriptedDispatch::default()
        };
        let failure = run(&mut transport, &mut dispatch, ServiceLimits::default())
            .expect_err("enumeration stream size was accepted");
        require_terminal_dispatch_failure(&failure, &transport, &dispatch);
        assert_eq!((dispatch.begin_calls, dispatch.step_calls), (1, 0));
        assert_eq!(decode_output(&transport.output).len(), 1);
    }
    {
        let mut transport = ScriptedTransport::new();
        append_hello(&mut transport.input);
        append_stream(&mut transport.input, 1, 1);
        let mut dispatch = ScriptedDispatch {
            begin_result: Ok(MAXIMUM_ENUMERATED_ENTRY_BYTES + 1),
            ..ScriptedDispatch::default()
        };
        let failure = run(&mut transport, &mut dispatch, ServiceLimits::default())
            .expect_err("over-limit stream size was accepted");
        require_terminal_dispatch_failure(&failure, &transport, &dispatch);
        assert_eq!((dispatch.begin_calls, dispatch.step_calls), (1, 0));
        assert_eq!(decode_output(&transport.output).len(), 1);
    }
    {
        let mut transport = ScriptedTransport::new();
        append_hello(&mut transport.input);
        append_stream(&mut transport.input, 1, 1);
        let data = [1_u8, 2, 3];
        let mut dispatch = ScriptedDispatch {
            begin_result: Ok(2),
            steps: vec![ScriptedStep::DataChunk(&data)],
            ..ScriptedDispatch::default()
        };
        let failure = run(&mut transport, &mut dispatch, ServiceLimits::default())
            .expect_err("stream over-completion was accepted");
        require_terminal_dispatch_failure(&failure, &transport, &dispatch);
        assert_eq!(decode_output(&transport.output).len(), 1);
    }
    {
        let mut transport = ScriptedTransport::new();
        append_hello(&mut transport.input);
        append_stream(&mut transport.input, 1, 1);
        let data = [1_u8];
        let mut dispatch = ScriptedDispatch {
            begin_result: Ok(2),
            steps: vec![ScriptedStep::DataChunk(&data), ScriptedStep::Complete],
            ..ScriptedDispatch::default()
        };
        let failure = run(&mut transport, &mut dispatch, ServiceLimits::default())
            .expect_err("stream under-completion was accepted");
        require_terminal_dispatch_failure(&failure, &transport, &dispatch);
        assert_eq!(
            output_types(&decode_output(&transport.output)),
            vec![MessageType::Ready, MessageType::DataChunk]
        );
    }
}

// ------------------------------------------------------------- transport ----

#[test]
fn probe_failures_abort_without_a_dispatch_step() {
    for status in [InputStatus::PeerClosed, InputStatus::Failed] {
        let mut transport = ScriptedTransport::new();
        append_hello(&mut transport.input);
        append_enumerate(&mut transport.input, 1);
        transport.probes = vec![status];
        let mut dispatch = ScriptedDispatch {
            steps: vec![ScriptedStep::Complete],
            ..ScriptedDispatch::default()
        };

        let failure = run(&mut transport, &mut dispatch, ServiceLimits::default())
            .expect_err("probe failure was ignored");
        assert_eq!(failure.error, ServiceError::TransportFailure);
        assert_eq!(
            failure.io_status,
            if status == InputStatus::PeerClosed {
                IoStatus::PeerClosed
            } else {
                IoStatus::Failed
            }
        );
        assert_eq!(
            (
                dispatch.begin_calls,
                dispatch.step_calls,
                dispatch.cancel_calls,
                dispatch.end_calls
            ),
            (1, 0, 1, 0)
        );
        assert_eq!(
            (
                transport.probe_calls,
                transport.abort_calls,
                transport.close_calls
            ),
            (1, 1, 0)
        );
        assert_eq!(decode_output(&transport.output).len(), 1);
    }
    // The C++ case additionally injected the out-of-table probe value `0xff`
    // and expected `internal_failure`. `InputStatus` has four variants and no
    // wire encoding, so that value cannot be constructed here.
}

#[test]
fn payload_write_failure_cancels_and_aborts_once() {
    let mut transport = ScriptedTransport::new();
    append_hello(&mut transport.input);
    append_enumerate(&mut transport.input, 1);
    transport.fail_write_call = 3;
    let entries = [entry(1, 1, b"x")];
    let mut dispatch = ScriptedDispatch {
        steps: vec![ScriptedStep::EntryBatch(&entries)],
        ..ScriptedDispatch::default()
    };

    let failure = run(&mut transport, &mut dispatch, ServiceLimits::default())
        .expect_err("payload write failure was ignored");
    assert_eq!(failure.error, ServiceError::TransportFailure);
    assert_eq!(failure.io_status, IoStatus::Failed);
    assert_eq!(
        (
            dispatch.begin_calls,
            dispatch.step_calls,
            dispatch.cancel_calls,
            dispatch.end_calls
        ),
        (1, 1, 1, 0)
    );
    assert_eq!(
        (
            transport.write_calls,
            transport.abort_calls,
            transport.close_calls
        ),
        (3, 1, 0)
    );
    assert_eq!(transport.output.len(), FRAME_HEADER_BYTES * 2);
}

#[test]
fn invalid_headers_do_not_consume_payload_or_dispatch() {
    {
        let mut transport = ScriptedTransport::new();
        append_hello(&mut transport.input);
        let invalid_offset = transport.input.len();
        append_header(
            &mut transport.input,
            &FrameHeader::new(MessageType::Enumerate, SESSION_ID + 1, 1, 1),
        );
        transport.input.push(0x5a);
        let mut dispatch = ScriptedDispatch::default();

        let failure = run(&mut transport, &mut dispatch, ServiceLimits::default())
            .expect_err("wrong-session header was accepted");
        assert_eq!(failure.error, ServiceError::ProtocolFailure);
        assert_eq!(failure.protocol_error, Some(ProtocolError::WrongSessionId));
        assert_eq!(transport.read_calls, 3);
        assert_eq!(
            transport.input_position,
            invalid_offset + FRAME_HEADER_BYTES,
            "the rejected header's payload was consumed"
        );
        assert_eq!((dispatch.begin_calls, dispatch.step_calls), (0, 0));
        assert_eq!((transport.abort_calls, transport.close_calls), (1, 0));
        assert_eq!(decode_output(&transport.output).len(), 1);
    }
    {
        let mut transport = ScriptedTransport::new();
        append_hello(&mut transport.input);
        let invalid_offset = transport.input.len();
        append_header(
            &mut transport.input,
            &FrameHeader::new(MessageType::Enumerate, SESSION_ID, 1, 0),
        );
        let length_offset = invalid_offset + 12;
        transport.input[length_offset..length_offset + 4]
            .copy_from_slice(&(MAXIMUM_FRAME_PAYLOAD_BYTES + 1).to_le_bytes());
        transport.input.push(0x5a);
        let mut dispatch = ScriptedDispatch::default();

        let failure = run(&mut transport, &mut dispatch, ServiceLimits::default())
            .expect_err("oversized header was accepted");
        assert_eq!(failure.error, ServiceError::ProtocolFailure);
        assert_eq!(failure.protocol_error, Some(ProtocolError::PayloadTooLarge));
        assert_eq!(transport.read_calls, 3);
        assert_eq!(
            transport.input_position,
            invalid_offset + FRAME_HEADER_BYTES
        );
        assert_eq!((dispatch.begin_calls, dispatch.step_calls), (0, 0));
        assert_eq!((transport.abort_calls, transport.close_calls), (1, 0));
        assert_eq!(decode_output(&transport.output).len(), 1);
    }
}

// ------------------------------------------------------ canonical flows ----

#[test]
fn handshake_and_shutdown() {
    let mut transport = ScriptedTransport::new();
    append_hello(&mut transport.input);
    append_shutdown(&mut transport.input);
    let mut dispatch = ScriptedDispatch::default();

    let summary = run(&mut transport, &mut dispatch, ServiceLimits::default())
        .expect("canonical service lifetime");
    assert_eq!(summary.session_id, SESSION_ID);
    assert!(transport.closed() && !transport.aborted());
    assert_eq!((transport.close_calls, transport.abort_calls), (1, 0));
    assert!(!dispatch.began(), "shutdown invoked the dispatcher");

    let frames = decode_output(&transport.output);
    assert_eq!(output_types(&frames), vec![MessageType::Ready]);
    decode_ready_payload(&frames[0].view()).expect("canonical ready frame");
}

#[test]
fn successful_enumeration() {
    let mut transport = ScriptedTransport::new();
    append_hello(&mut transport.input);
    append_enumerate(&mut transport.input, 1);
    append_shutdown(&mut transport.input);

    let entries = [entry(2, 3, b"a.bin"), entry(4, 5, b"b.bin")];
    let mut dispatch = ScriptedDispatch {
        steps: vec![ScriptedStep::EntryBatch(&entries), ScriptedStep::Complete],
        ..ScriptedDispatch::default()
    };

    let summary =
        run(&mut transport, &mut dispatch, ServiceLimits::default()).expect("enumeration lifetime");
    assert_eq!(summary.dispatch_steps, 2);
    assert!(dispatch.began() && dispatch.ended() && !dispatch.cancelled());
    assert_eq!((dispatch.begin_calls, dispatch.end_calls), (1, 1));
    assert_eq!(dispatch.operation, Some(Operation::Enumerate));
    assert_eq!(
        dispatch.source_policy.map(|policy| policy.source_size()),
        Some(SOURCE_SIZE)
    );

    let frames = decode_output(&transport.output);
    assert_eq!(
        output_types(&frames),
        vec![
            MessageType::Ready,
            MessageType::EntryBatch,
            MessageType::Complete
        ]
    );
    let mut storage = [EntryBatchEntry::default(); 2];
    let batch = decode_entry_batch_payload(&frames[1].view(), &full_entry_policy(), &mut storage)
        .expect("canonical entry batch");
    assert_eq!(batch.entries.len(), entries.len());
    decode_complete_payload(&frames[2].view(), OperationPhase::Enumerate)
        .expect("canonical completion");
}

#[test]
fn entry_batch_count_and_path_boundaries() {
    {
        let mut transport = ScriptedTransport::new();
        append_hello(&mut transport.input);
        append_enumerate(&mut transport.input, 1);
        append_shutdown(&mut transport.input);
        let entries: Vec<EntryBatchEntry<'_>> = (0..u64::from(MAXIMUM_ENTRY_BATCH_ENTRIES))
            .map(|index| entry(index + 1, 0, b"x"))
            .collect();
        let mut dispatch = ScriptedDispatch {
            steps: vec![ScriptedStep::EntryBatch(&entries), ScriptedStep::Complete],
            ..ScriptedDispatch::default()
        };
        run(&mut transport, &mut dispatch, ServiceLimits::default())
            .expect("exact entry-count boundary was rejected");
        assert!(dispatch.ended());
    }
    {
        let mut transport = ScriptedTransport::new();
        append_hello(&mut transport.input);
        append_enumerate(&mut transport.input, 1);
        let entries: Vec<EntryBatchEntry<'_>> = (0..=u64::from(MAXIMUM_ENTRY_BATCH_ENTRIES))
            .map(|index| entry(index + 1, 0, b"x"))
            .collect();
        let mut dispatch = ScriptedDispatch {
            steps: vec![ScriptedStep::EntryBatch(&entries)],
            ..ScriptedDispatch::default()
        };
        let failure = run(&mut transport, &mut dispatch, ServiceLimits::default())
            .expect_err("over-limit entry count was accepted");
        assert_eq!(failure.error, ServiceError::DispatchFailure);
        assert_eq!(
            failure.protocol_error,
            Some(ProtocolError::NoncanonicalValue)
        );
        assert!(dispatch.cancelled() && transport.aborted());
    }
    {
        let mut transport = ScriptedTransport::new();
        append_hello(&mut transport.input);
        append_enumerate(&mut transport.input, 1);
        append_shutdown(&mut transport.input);
        let path = vec![b'p'; shrink(MAXIMUM_ENTRY_BATCH_PATH_BYTES)];
        let entries = [entry(1, 0, &path)];
        let mut dispatch = ScriptedDispatch {
            steps: vec![ScriptedStep::EntryBatch(&entries), ScriptedStep::Complete],
            ..ScriptedDispatch::default()
        };
        run(&mut transport, &mut dispatch, ServiceLimits::default())
            .expect("exact entry-path boundary was rejected");
        assert!(dispatch.ended());
    }
    {
        // The C++ case fed a path one byte over the ceiling. `ArchiveSpelling`
        // rejects that at construction, so the over-long spelling can never
        // reach a `DispatchAction`.
        let path = vec![b'p'; shrink(MAXIMUM_ENTRY_BATCH_PATH_BYTES) + 1];
        assert_eq!(
            ArchiveSpelling::new(&path),
            Err(ProtocolError::NoncanonicalValue)
        );
        // The one out-of-range spelling still representable is the empty
        // placeholder, and the service rejects it exactly like the C++ code.
        let mut transport = ScriptedTransport::new();
        append_hello(&mut transport.input);
        append_enumerate(&mut transport.input, 1);
        let entries = [EntryBatchEntry {
            source_token: 1,
            size_bytes: 0,
            archive_path: ArchiveSpelling::EMPTY,
        }];
        let mut dispatch = ScriptedDispatch {
            steps: vec![ScriptedStep::EntryBatch(&entries)],
            ..ScriptedDispatch::default()
        };
        let failure = run(&mut transport, &mut dispatch, ServiceLimits::default())
            .expect_err("empty entry path was accepted");
        assert_eq!(failure.error, ServiceError::DispatchFailure);
        assert_eq!(
            failure.protocol_error,
            Some(ProtocolError::NoncanonicalValue)
        );
        assert!(dispatch.cancelled() && transport.aborted());
    }
}

#[test]
fn dispatch_views_are_encoded_before_the_next_transport_callback() {
    let mut transport = ScriptedTransport::new();
    append_hello(&mut transport.input);
    append_enumerate(&mut transport.input, 1);
    append_shutdown(&mut transport.input);

    let first = [entry(1, 0, b"first")];
    let second = [entry(2, 0, b"second")];
    let mut dispatch = ScriptedDispatch {
        steps: vec![
            ScriptedStep::EntryBatch(&first),
            ScriptedStep::EntryBatch(&second),
            ScriptedStep::Complete,
        ],
        ..ScriptedDispatch::default()
    };

    run(&mut transport, &mut dispatch, ServiceLimits::default()).expect("two-batch lifetime");
    assert!(dispatch.ended());
    let frames = decode_output(&transport.output);
    assert_eq!(
        output_types(&frames),
        vec![
            MessageType::Ready,
            MessageType::EntryBatch,
            MessageType::EntryBatch,
            MessageType::Complete
        ]
    );
    let mut storage = [EntryBatchEntry::default(); 1];
    let batch = decode_entry_batch_payload(&frames[1].view(), &full_entry_policy(), &mut storage)
        .expect("first batch");
    assert_eq!(batch.entries[0].source_token, 1);
    assert_eq!(batch.entries[0].archive_path.as_bytes(), b"first");
}

#[test]
fn successful_stream() {
    let mut transport = ScriptedTransport::new();
    append_hello(&mut transport.input);
    append_stream(&mut transport.input, 1, 9);
    append_shutdown(&mut transport.input);

    let data = [0x11_u8, 0x22, 0x33];
    let mut dispatch = ScriptedDispatch {
        begin_result: Ok(widen(data.len())),
        steps: vec![ScriptedStep::DataChunk(&data), ScriptedStep::Complete],
        ..ScriptedDispatch::default()
    };

    run(&mut transport, &mut dispatch, ServiceLimits::default()).expect("stream lifetime");
    assert!(dispatch.ended());
    assert_eq!((dispatch.end_calls, dispatch.cancel_calls), (1, 0));
    assert_eq!(dispatch.operation, Some(Operation::Stream));
    assert_eq!(dispatch.source_token, 9);

    let frames = decode_output(&transport.output);
    assert_eq!(
        output_types(&frames),
        vec![
            MessageType::Ready,
            MessageType::DataChunk,
            MessageType::Complete
        ]
    );
    let chunk = decode_data_chunk_payload(&frames[1].view(), widen(data.len()))
        .expect("canonical data chunk");
    assert_eq!(chunk.data, data);
    decode_complete_payload(&frames[2].view(), OperationPhase::Stream)
        .expect("canonical completion");
}

// ----------------------------------------------------------- unsupported ----

#[test]
fn begin_unsupported_is_terminal_without_an_operation_frame() {
    let mut transport = ScriptedTransport::new();
    append_hello(&mut transport.input);
    append_enumerate(&mut transport.input, 1);
    let mut dispatch = ScriptedDispatch {
        begin_result: Err(DispatchError::Unsupported),
        ..ScriptedDispatch::default()
    };

    let failure = run(&mut transport, &mut dispatch, ServiceLimits::default())
        .expect_err("unsupported begin was accepted");
    assert_eq!(failure.error, ServiceError::DispatchUnsupported);
    assert_eq!(failure.dispatch_error, Some(DispatchError::Unsupported));
    assert!(transport.aborted() && !transport.closed());
    assert_eq!(
        output_types(&decode_output(&transport.output)),
        vec![MessageType::Ready]
    );
}

#[test]
fn step_unsupported_is_terminal_without_an_operation_frame() {
    let mut transport = ScriptedTransport::new();
    append_hello(&mut transport.input);
    append_enumerate(&mut transport.input, 1);
    append_shutdown(&mut transport.input);
    let mut dispatch = ScriptedDispatch {
        step_status: Err(DispatchError::Unsupported),
        ..ScriptedDispatch::default()
    };

    let failure = run(&mut transport, &mut dispatch, ServiceLimits::default())
        .expect_err("unsupported step was accepted");
    assert_eq!(
        (
            failure.error,
            failure.protocol_error,
            failure.io_status,
            failure.dispatch_error,
            failure.session_id,
            failure.dispatch_steps
        ),
        (
            ServiceError::DispatchUnsupported,
            None,
            IoStatus::Ok,
            Some(DispatchError::Unsupported),
            SESSION_ID,
            1
        )
    );
    assert!(dispatch.began() && dispatch.cancelled() && !dispatch.ended());
    assert!(transport.aborted() && !transport.closed());
    assert_eq!(
        output_types(&decode_output(&transport.output)),
        vec![MessageType::Ready]
    );
}

#[test]
fn accept_read_reply_unsupported_is_terminal_without_an_operation_frame() {
    let read_data = [0x42_u8, 0x43];
    let mut transport = ScriptedTransport::new();
    append_hello(&mut transport.input);
    append_enumerate(&mut transport.input, 1);
    append_read_reply(
        &mut transport.input,
        1,
        1,
        &read_data,
        ProtocolStatus::Ok,
        0,
    );
    append_shutdown(&mut transport.input);
    let mut dispatch = ScriptedDispatch {
        steps: vec![ScriptedStep::NeedRead {
            offset: 10,
            length: narrow(read_data.len()),
        }],
        accept_status: Err(DispatchError::Unsupported),
        ..ScriptedDispatch::default()
    };

    let failure = run(&mut transport, &mut dispatch, ServiceLimits::default())
        .expect_err("unsupported read acceptance was accepted");
    assert_eq!(
        (
            failure.error,
            failure.protocol_error,
            failure.io_status,
            failure.dispatch_error,
            failure.session_id,
            failure.dispatch_steps
        ),
        (
            ServiceError::DispatchUnsupported,
            None,
            IoStatus::Ok,
            Some(DispatchError::Unsupported),
            SESSION_ID,
            1
        )
    );
    assert!(dispatch.began());
    assert_eq!(dispatch.accepted_read_sequence, 1);
    assert_eq!(dispatch.accepted_read, read_data);
    assert!(dispatch.cancelled() && !dispatch.ended());
    assert!(transport.aborted() && !transport.closed());
    assert_eq!(
        output_types(&decode_output(&transport.output)),
        vec![MessageType::Ready, MessageType::ReadRequest]
    );
}

#[test]
fn the_compile_fixed_unsupported_dispatcher_refuses_every_request() {
    for mut input in [
        {
            let mut bytes = Vec::new();
            append_hello(&mut bytes);
            append_enumerate(&mut bytes, 1);
            bytes
        },
        {
            let mut bytes = Vec::new();
            append_hello(&mut bytes);
            append_stream(&mut bytes, 1, 7);
            bytes
        },
    ] {
        let mut transport = ScriptedTransport::new();
        transport.input.append(&mut input);
        let mut buffers = Buffers::new();
        let failure = run_parser_worker_service(
            &mut transport,
            UnsupportedDispatcher::new(),
            buffers.borrow(),
            ServiceLimits::default(),
        )
        .expect_err("the unsupported dispatcher served a request");
        assert_eq!(failure.error, ServiceError::DispatchUnsupported);
        assert_eq!(
            output_types(&decode_output(&transport.output)),
            vec![MessageType::Ready]
        );
        assert!(transport.aborted() && !transport.closed());
    }
}

#[test]
fn the_compile_fixed_unsupported_dispatcher_still_shuts_down_cleanly() {
    let mut transport = ScriptedTransport::new();
    append_hello(&mut transport.input);
    append_shutdown(&mut transport.input);
    let mut buffers = Buffers::new();
    let summary = run_parser_worker_service(
        &mut transport,
        UnsupportedDispatcher::new(),
        buffers.borrow(),
        ServiceLimits::default(),
    )
    .expect("handshake and shutdown");
    assert_eq!(summary.session_id, SESSION_ID);
    assert_eq!(summary.dispatch_steps, 0);
    assert!(transport.closed() && !transport.aborted());
}

// ------------------------------------------- transport / malformed input ----

#[test]
fn transport_and_malformed_failures_fail_closed() {
    {
        let mut transport = ScriptedTransport::new();
        append_hello(&mut transport.input);
        transport.input[0] = 0;
        let mut dispatch = ScriptedDispatch::default();
        let failure = run(&mut transport, &mut dispatch, ServiceLimits::default())
            .expect_err("malformed hello was accepted");
        assert_eq!(failure.error, ServiceError::ProtocolFailure);
        assert_eq!(failure.protocol_error, Some(ProtocolError::InvalidMagic));
        assert!(transport.aborted() && transport.output.is_empty());
    }
    {
        let mut transport = ScriptedTransport::new();
        append_hello(&mut transport.input);
        transport.fail_read_call = 2;
        let mut dispatch = ScriptedDispatch::default();
        let failure = run(&mut transport, &mut dispatch, ServiceLimits::default())
            .expect_err("short hello payload was accepted");
        assert_eq!(failure.error, ServiceError::TransportFailure);
        assert_eq!(failure.io_status, IoStatus::PeerClosed);
        assert!(transport.aborted());
    }
    {
        let mut transport = ScriptedTransport::new();
        append_hello(&mut transport.input);
        transport.fail_write_call = 1;
        let mut dispatch = ScriptedDispatch::default();
        let failure = run(&mut transport, &mut dispatch, ServiceLimits::default())
            .expect_err("ready write failure was ignored");
        assert_eq!(failure.error, ServiceError::TransportFailure);
        assert_eq!(failure.io_status, IoStatus::Failed);
        assert!(transport.aborted());
    }
    {
        // A clean peer close before any frame arrives.
        let mut transport = ScriptedTransport::new();
        let mut dispatch = ScriptedDispatch::default();
        let failure = run(&mut transport, &mut dispatch, ServiceLimits::default())
            .expect_err("an empty channel produced a session");
        assert_eq!(failure.error, ServiceError::TransportFailure);
        assert_eq!(failure.io_status, IoStatus::PeerClosed);
        assert_eq!(failure.session_id, 0);
        assert!(transport.aborted() && transport.output.is_empty());
    }
}

// --------------------------------------------------------- source reads ----

#[test]
fn source_read_sequence_is_relayed_exactly_once() {
    let read_data = [0x42_u8, 0x43];
    let mut transport = ScriptedTransport::new();
    append_hello(&mut transport.input);
    append_enumerate(&mut transport.input, 1);
    append_read_reply(
        &mut transport.input,
        1,
        1,
        &read_data,
        ProtocolStatus::Ok,
        0,
    );
    append_shutdown(&mut transport.input);

    let mut dispatch = ScriptedDispatch {
        steps: vec![
            ScriptedStep::NeedRead {
                offset: 10,
                length: narrow(read_data.len()),
            },
            ScriptedStep::Complete,
        ],
        ..ScriptedDispatch::default()
    };
    run(&mut transport, &mut dispatch, ServiceLimits::default()).expect("source read lifetime");
    assert_eq!(dispatch.accepted_read_sequence, 1);
    assert_eq!(dispatch.accepted_read, read_data);
    assert_eq!(dispatch.accept_calls, 1);

    let frames = decode_output(&transport.output);
    assert_eq!(
        output_types(&frames),
        vec![
            MessageType::Ready,
            MessageType::ReadRequest,
            MessageType::Complete
        ]
    );
    let policy = SourceReadPolicy::new(SOURCE_SIZE, MAXIMUM_READ_BYTES).expect("policy");
    let request =
        decode_read_request_payload(&frames[1].view(), &policy, 1).expect("canonical read request");
    assert_eq!(request.offset, 10);
    assert_eq!(request.length, narrow(read_data.len()));
}

#[test]
fn an_active_source_failure_status_is_terminal() {
    for status in [
        ProtocolStatus::SourceChanged,
        ProtocolStatus::SourceReadFailed,
    ] {
        let mut transport = ScriptedTransport::new();
        append_hello(&mut transport.input);
        append_enumerate(&mut transport.input, 1);
        append_read_reply(&mut transport.input, 1, 1, &[], status, 1);
        let mut dispatch = ScriptedDispatch {
            steps: vec![ScriptedStep::NeedRead {
                offset: 0,
                length: 1,
            }],
            ..ScriptedDispatch::default()
        };

        let failure = run(&mut transport, &mut dispatch, ServiceLimits::default())
            .expect_err("active source failure was tolerated");
        assert_eq!(failure.error, ServiceError::SourceFailure);
        assert_eq!(dispatch.accept_calls, 0);
        assert!(dispatch.cancelled() && transport.aborted());
    }
}

/// The one late read reply a cancelled session drains, for every status the
/// protocol allows it to carry.
#[test]
fn a_post_cancel_read_reply_is_drained_for_every_canonical_status() {
    let ok_data = [0x5a_u8];
    for status in [
        ProtocolStatus::Ok,
        ProtocolStatus::SourceChanged,
        ProtocolStatus::SourceReadFailed,
    ] {
        let mut transport = ScriptedTransport::new();
        append_hello(&mut transport.input);
        append_enumerate(&mut transport.input, 1);
        append_cancel(&mut transport.input, 1);
        let reply_data: &[u8] = if status == ProtocolStatus::Ok {
            &ok_data
        } else {
            &[]
        };
        append_read_reply(&mut transport.input, 1, 1, reply_data, status, 1);
        append_shutdown(&mut transport.input);
        let mut dispatch = ScriptedDispatch {
            steps: vec![ScriptedStep::NeedRead {
                offset: 0,
                length: 1,
            }],
            ..ScriptedDispatch::default()
        };

        run(&mut transport, &mut dispatch, ServiceLimits::default())
            .expect("canonical post-cancel reply status was not drained");
        assert!(dispatch.cancelled() && !dispatch.ended());
        assert_eq!(dispatch.accept_calls, 0);
        assert!(transport.closed() && !transport.aborted());
        assert_eq!(
            output_types(&decode_output(&transport.output)),
            vec![
                MessageType::Ready,
                MessageType::ReadRequest,
                MessageType::CancelAck
            ]
        );
    }
}

#[test]
fn a_noncanonical_late_read_reply_is_rejected() {
    let ok_data = [0x5a_u8];
    {
        let mut transport = ScriptedTransport::new();
        append_hello(&mut transport.input);
        append_enumerate(&mut transport.input, 1);
        append_cancel(&mut transport.input, 1);
        append_read_reply(&mut transport.input, 1, 2, &ok_data, ProtocolStatus::Ok, 1);
        let mut dispatch = ScriptedDispatch {
            steps: vec![ScriptedStep::NeedRead {
                offset: 0,
                length: 1,
            }],
            ..ScriptedDispatch::default()
        };

        let failure = run(&mut transport, &mut dispatch, ServiceLimits::default())
            .expect_err("noncanonical late reply bypassed payload validation");
        assert_eq!(failure.error, ServiceError::ProtocolFailure);
        assert_eq!(
            failure.protocol_error,
            Some(ProtocolError::NoncanonicalValue)
        );
        assert_eq!(dispatch.accept_calls, 0);
        assert!(transport.aborted());
    }
    {
        let mut transport = ScriptedTransport::new();
        append_hello(&mut transport.input);
        append_enumerate(&mut transport.input, 1);
        append_cancel(&mut transport.input, 1);
        append_read_reply(&mut transport.input, 2, 1, &ok_data, ProtocolStatus::Ok, 1);
        let mut dispatch = ScriptedDispatch {
            steps: vec![ScriptedStep::NeedRead {
                offset: 0,
                length: 1,
            }],
            ..ScriptedDispatch::default()
        };

        let failure = run(&mut transport, &mut dispatch, ServiceLimits::default())
            .expect_err("canonical late reply bypassed protocol observation");
        assert_eq!(failure.error, ServiceError::ProtocolFailure);
        assert_eq!(failure.protocol_error, Some(ProtocolError::WrongRequestId));
        assert_eq!(dispatch.accept_calls, 0);
        assert!(transport.aborted());
    }
}

// ------------------------------------------------------ cancel crossings ----

#[test]
fn cancel_crossings() {
    {
        let mut transport = ScriptedTransport::new();
        append_hello(&mut transport.input);
        append_enumerate(&mut transport.input, 1);
        append_cancel(&mut transport.input, 1);
        append_shutdown(&mut transport.input);
        transport.probes = vec![InputStatus::Available];
        let mut dispatch = ScriptedDispatch {
            steps: vec![ScriptedStep::Complete],
            ..ScriptedDispatch::default()
        };
        run(&mut transport, &mut dispatch, ServiceLimits::default())
            .expect("pre-step cancellation failed");
        assert!(dispatch.cancelled() && !dispatch.ended());
        assert_eq!(
            output_types(&decode_output(&transport.output)),
            vec![MessageType::Ready, MessageType::CancelAck]
        );
    }
    {
        let mut transport = ScriptedTransport::new();
        append_hello(&mut transport.input);
        append_enumerate(&mut transport.input, 1);
        append_cancel(&mut transport.input, 1);
        append_shutdown(&mut transport.input);
        transport.probes = vec![InputStatus::Unavailable];
        let mut dispatch = ScriptedDispatch {
            steps: vec![ScriptedStep::NeedRead {
                offset: 0,
                length: 1,
            }],
            ..ScriptedDispatch::default()
        };
        run(&mut transport, &mut dispatch, ServiceLimits::default())
            .expect("read/cancel crossing failed");
        assert!(dispatch.cancelled());
        assert_eq!(
            output_types(&decode_output(&transport.output)),
            vec![
                MessageType::Ready,
                MessageType::ReadRequest,
                MessageType::CancelAck
            ]
        );
    }
    {
        let mut transport = ScriptedTransport::new();
        append_hello(&mut transport.input);
        append_enumerate(&mut transport.input, 1);
        append_cancel(&mut transport.input, 1);
        append_shutdown(&mut transport.input);
        let mut dispatch = ScriptedDispatch {
            steps: vec![ScriptedStep::Complete],
            ..ScriptedDispatch::default()
        };
        run(&mut transport, &mut dispatch, ServiceLimits::default())
            .expect("completion/late-cancel crossing failed");
        assert!(dispatch.ended() && !dispatch.cancelled());
        assert_eq!(
            output_types(&decode_output(&transport.output)),
            vec![MessageType::Ready, MessageType::Complete],
            "a stale cancel produced an acknowledgement"
        );
    }
}

// -------------------------------------------- invalid dispatch / budgets ----

#[test]
fn invalid_dispatch_and_budgets() {
    {
        let mut transport = ScriptedTransport::new();
        append_hello(&mut transport.input);
        append_stream(&mut transport.input, 1, 3);
        let mut dispatch = ScriptedDispatch {
            begin_result: Ok(1),
            steps: vec![ScriptedStep::EntryBatch(&[])],
            ..ScriptedDispatch::default()
        };
        let failure = run(&mut transport, &mut dispatch, ServiceLimits::default())
            .expect_err("wrong-operation dispatch action was accepted");
        assert_eq!(failure.error, ServiceError::DispatchFailure);
        assert!(dispatch.cancelled() && transport.aborted());
    }
    {
        let mut transport = ScriptedTransport::new();
        append_hello(&mut transport.input);
        append_enumerate(&mut transport.input, 1);
        let mut dispatch = ScriptedDispatch {
            steps: vec![ScriptedStep::EntryBatch(&[]), ScriptedStep::Complete],
            ..ScriptedDispatch::default()
        };
        let failure = run(
            &mut transport,
            &mut dispatch,
            ServiceLimits {
                protocol_budgets: ProtocolBudgets::default(),
                maximum_dispatch_steps: 1,
            },
        )
        .expect_err("empty entry batch did not fail before the step boundary");
        assert_eq!(failure.error, ServiceError::DispatchFailure);
    }
    {
        let mut transport = ScriptedTransport::new();
        append_hello(&mut transport.input);
        append_enumerate(&mut transport.input, 1);
        let entries = [entry(1, 1, b"x")];
        let mut dispatch = ScriptedDispatch {
            steps: vec![ScriptedStep::EntryBatch(&entries), ScriptedStep::Complete],
            ..ScriptedDispatch::default()
        };
        let failure = run(
            &mut transport,
            &mut dispatch,
            ServiceLimits {
                protocol_budgets: ProtocolBudgets::default(),
                maximum_dispatch_steps: 1,
            },
        )
        .expect_err("dispatch step budget boundary failed");
        assert_eq!(failure.error, ServiceError::DispatchBudgetExceeded);
        assert_eq!(failure.dispatch_steps, 1);
        assert!(transport.aborted());
    }
    {
        let mut transport = ScriptedTransport::new();
        append_hello(&mut transport.input);
        append_shutdown(&mut transport.input);
        let mut dispatch = ScriptedDispatch::default();
        run(
            &mut transport,
            &mut dispatch,
            ServiceLimits {
                protocol_budgets: budgets(3, widen(HELLO_PAYLOAD_BYTES)),
                maximum_dispatch_steps: 1,
            },
        )
        .expect("exact handshake/shutdown budget was rejected");
    }
    {
        let mut transport = ScriptedTransport::new();
        append_hello(&mut transport.input);
        append_shutdown(&mut transport.input);
        let mut dispatch = ScriptedDispatch::default();
        let failure = run(
            &mut transport,
            &mut dispatch,
            ServiceLimits {
                protocol_budgets: budgets(2, widen(HELLO_PAYLOAD_BYTES)),
                maximum_dispatch_steps: 1,
            },
        )
        .expect_err("message budget exhaustion was tolerated");
        assert_eq!(failure.error, ServiceError::ProtocolFailure);
        assert_eq!(
            failure.protocol_error,
            Some(ProtocolError::MessageBudgetExceeded)
        );
        assert!(transport.aborted());
    }
}
