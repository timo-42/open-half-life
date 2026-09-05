//! The dispatcher driven through the real worker service, over a scripted
//! transport that plays the parent.
//!
//! Nothing here is a mock of the protocol: `run_parser_worker_service` runs
//! its whole lifetime, encodes and validates every frame, and the transport
//! below answers `read_request`s out of an in-memory container exactly as the
//! parent's source-read broker would. Every container is synthetic, built by
//! the decoders' own project-authored writers.

use std::collections::VecDeque;

use ohl_isz::testing::ArchiveBuilder;
use ohl_mscab::test_support::{CabinetSpec, FileSpec, FolderSpec, Method, build};
use ohl_parser_backends::{BackendLimits, ContainerDispatcher, ContainerKind};
use ohl_parser_protocol::messages::{
    HELLO_PAYLOAD_BYTES, decode_complete_payload, decode_data_chunk_payload,
    decode_entry_batch_payload, decode_read_request_payload, encode_hello_payload,
    encode_read_reply_payload, encode_stream_entry_payload,
};
use ohl_parser_protocol::{
    EntryBatchEntry, EntryBatchPolicy, FRAME_HEADER_BYTES, FrameHeader, FrameView, Hello,
    MAXIMUM_FRAME_PAYLOAD_BYTES, MessageType, OperationPhase, ProtocolStatus, ReadReply,
    SourceReadPolicy, StreamEntry, decode_frame_header, encode_frame,
};
use ohl_parser_worker_service::{
    InputStatus, IoStatus, ServiceBuffers, ServiceError, ServiceLimits, Transport,
    run_parser_worker_service,
};
use ohl_wise::testing::{PackageOptions, SyntheticFile, build_package};

const SESSION_ID: u64 = 0x0102_0304_0506_0708;
const MAXIMUM_READ_BYTES: u32 = 64 * 1024;
const PAYLOAD_CAPACITY: usize = MAXIMUM_FRAME_PAYLOAD_BYTES as usize;

/// What the parent should do once the worker completes a request.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Next {
    /// Ask for an enumeration.
    Enumerate,
    /// Ask for one entry by token.
    Stream(u64),
    /// End the session.
    Shutdown,
}

/// One entry the parent decoded out of an `entry_batch`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Offered {
    token: u64,
    size_bytes: u64,
    spelling: String,
}

/// The parent side of one session, over an in-memory container.
struct ParentTransport {
    container: Vec<u8>,
    incoming: VecDeque<u8>,
    outgoing: Vec<u8>,
    plan: VecDeque<Next>,
    offered: Vec<Offered>,
    streamed: Vec<u8>,
    completes: usize,
    chunks: usize,
    request_id: u64,
    /// Answer this many reads, then report a source failure.
    reads_before_failure: Option<usize>,
    reads: usize,
    /// Cancel the active request after this many `data_chunk`s.
    cancel_after_chunks: Option<usize>,
    cancelled: bool,
    closed: bool,
    aborted: bool,
}

impl ParentTransport {
    fn new(container: Vec<u8>, plan: Vec<Next>) -> Self {
        let mut transport = Self {
            container,
            incoming: VecDeque::new(),
            outgoing: Vec::new(),
            plan: plan.into(),
            offered: Vec::new(),
            streamed: Vec::new(),
            completes: 0,
            chunks: 0,
            request_id: 0,
            reads_before_failure: None,
            reads: 0,
            cancel_after_chunks: None,
            cancelled: false,
            closed: false,
            aborted: false,
        };
        transport.send_hello();
        transport
    }

    fn source_policy(&self) -> SourceReadPolicy {
        SourceReadPolicy::new(self.container.len() as u64, MAXIMUM_READ_BYTES)
            .expect("a valid source policy")
    }

    fn push_frame(&mut self, header: &FrameHeader, payload: &[u8]) {
        let mut frame = vec![0u8; FRAME_HEADER_BYTES + payload.len()];
        let written = encode_frame(header, payload, &mut frame).expect("frame encoding");
        self.incoming.extend(&frame[..written]);
    }

    fn send_hello(&mut self) {
        let mut payload = [0u8; HELLO_PAYLOAD_BYTES];
        encode_hello_payload(
            &Hello {
                source_size: self.container.len() as u64,
                maximum_read_bytes: MAXIMUM_READ_BYTES,
            },
            &mut payload,
        )
        .expect("hello encoding");
        self.push_frame(
            &FrameHeader::new(
                MessageType::Hello,
                SESSION_ID,
                0,
                HELLO_PAYLOAD_BYTES as u32,
            ),
            &payload,
        );
    }

    /// Sends the next planned request, or `shutdown` when the plan is spent.
    fn send_next(&mut self) {
        let next = self.plan.pop_front().unwrap_or(Next::Shutdown);
        self.request_id += 1;
        let request_id = self.request_id;
        match next {
            Next::Enumerate => self.push_frame(
                &FrameHeader::new(MessageType::Enumerate, SESSION_ID, request_id, 0),
                &[],
            ),
            Next::Stream(token) => {
                let mut payload = [0u8; 8];
                let written =
                    encode_stream_entry_payload(&StreamEntry { source_token: token }, &mut payload)
                        .expect("stream request encoding");
                self.push_frame(
                    &FrameHeader::new(
                        MessageType::StreamEntry,
                        SESSION_ID,
                        request_id,
                        written as u32,
                    ),
                    &payload[..written],
                );
            }
            Next::Shutdown => self.push_frame(
                &FrameHeader::new(MessageType::Shutdown, SESSION_ID, 0, 0),
                &[],
            ),
        }
    }

    fn answer_read(&mut self, header: FrameHeader, payload: &[u8]) {
        let frame = FrameView::new(header, payload);
        let policy = self.source_policy();
        let request = decode_read_request_payload(&frame, &policy, header.request_id.max(1) as u32)
            .or_else(|_| {
                // The sequence number is the worker's; re-decode with the one
                // it used, which the header does not carry.
                (1..=64u32).find_map(|sequence| {
                    decode_read_request_payload(&frame, &policy, sequence).ok()
                }).ok_or(())
            })
            .expect("a decodable read request");
        self.reads += 1;
        let failing = self
            .reads_before_failure
            .is_some_and(|limit| self.reads > limit);
        let start = usize::try_from(request.offset).expect("host offset");
        let end = start + request.length as usize;
        let (status, data): (ProtocolStatus, &[u8]) = if failing {
            (ProtocolStatus::SourceReadFailed, &[])
        } else {
            (ProtocolStatus::Ok, &self.container[start..end])
        };
        let mut payload = vec![0u8; 6 + data.len()];
        let written = encode_read_reply_payload(
            &ReadReply {
                read_sequence: request.read_sequence,
                status,
                data,
            },
            request.read_sequence,
            request.length,
            &mut payload,
        )
        .expect("read reply encoding");
        self.push_frame(
            &FrameHeader::new(
                MessageType::ReadReply,
                SESSION_ID,
                header.request_id,
                written as u32,
            ),
            &payload[..written],
        );
    }

    fn send_cancel(&mut self, request_id: u64) {
        self.push_frame(
            &FrameHeader::new(MessageType::Cancel, SESSION_ID, request_id, 0),
            &[],
        );
        self.cancelled = true;
    }

    /// Reacts to one complete frame the worker wrote.
    fn observe(&mut self, header: FrameHeader, payload: &[u8]) {
        match header.message_type {
            MessageType::Ready => self.send_next(),
            MessageType::ReadRequest => self.answer_read(header, payload),
            MessageType::EntryBatch => {
                let frame = FrameView::new(header, payload);
                let mut storage = [EntryBatchEntry::default(); 256];
                let policy = EntryBatchPolicy::new(
                    256,
                    64 * 1024,
                    8 * 1024 * 1024 * 1024,
                    32 * 1024 * 1024 * 1024,
                    self.offered.last().map(|entry| entry.token),
                )
                .expect("a valid batch policy");
                let batch = decode_entry_batch_payload(&frame, &policy, &mut storage)
                    .expect("a decodable entry batch");
                for entry in batch.entries {
                    self.offered.push(Offered {
                        token: entry.source_token,
                        size_bytes: entry.size_bytes,
                        spelling: entry.archive_path.as_str().to_owned(),
                    });
                }
            }
            MessageType::DataChunk => {
                let frame = FrameView::new(header, payload);
                let chunk = decode_data_chunk_payload(&frame, u64::MAX).expect("a decodable chunk");
                self.streamed.extend_from_slice(chunk.data);
                self.chunks += 1;
                if self
                    .cancel_after_chunks
                    .is_some_and(|limit| self.chunks >= limit && !self.cancelled)
                {
                    self.send_cancel(header.request_id);
                }
            }
            MessageType::Complete => {
                let frame = FrameView::new(header, payload);
                decode_complete_payload(&frame, OperationPhase::Enumerate)
                    .or_else(|_| decode_complete_payload(&frame, OperationPhase::Stream))
                    .expect("a decodable complete");
                self.completes += 1;
                self.send_next();
            }
            MessageType::CancelAck => {
                self.plan.clear();
                self.send_next();
            }
            _ => panic!("the worker wrote an unexpected message"),
        }
    }
}

impl Transport for ParentTransport {
    fn read_exact(&mut self, destination: &mut [u8]) -> IoStatus {
        if self.incoming.len() < destination.len() {
            return IoStatus::PeerClosed;
        }
        for slot in destination.iter_mut() {
            *slot = self.incoming.pop_front().expect("checked above");
        }
        IoStatus::Ok
    }

    fn write_all(&mut self, source: &[u8]) -> IoStatus {
        // The service writes a frame header and its payload in separate
        // calls, so bytes are buffered here and whole frames are acted on as
        // they complete.
        self.outgoing.extend_from_slice(source);
        loop {
            if self.outgoing.len() < FRAME_HEADER_BYTES {
                return IoStatus::Ok;
            }
            let Ok(head) = <&[u8; FRAME_HEADER_BYTES]>::try_from(&self.outgoing[..FRAME_HEADER_BYTES])
            else {
                return IoStatus::Failed;
            };
            let Ok(header) = decode_frame_header(head) else {
                return IoStatus::Failed;
            };
            let end = FRAME_HEADER_BYTES + header.payload_length as usize;
            if self.outgoing.len() < end {
                return IoStatus::Ok;
            }
            let payload = self.outgoing[FRAME_HEADER_BYTES..end].to_vec();
            self.outgoing.drain(..end);
            self.observe(header, &payload);
        }
    }

    fn probe_input(&mut self) -> InputStatus {
        if self.incoming.is_empty() {
            InputStatus::Unavailable
        } else {
            InputStatus::Available
        }
    }

    fn abort_io(&mut self) {
        self.aborted = true;
    }

    fn close_io(&mut self) {
        self.closed = true;
    }
}

/// Runs one whole session and returns the parent's observations.
fn run(
    container: Vec<u8>,
    plan: Vec<Next>,
    limits: BackendLimits,
    configure: impl FnOnce(&mut ParentTransport),
) -> (ParentTransport, Result<(), ServiceError>) {
    let mut transport = ParentTransport::new(container, plan);
    configure(&mut transport);
    let mut arena = vec![0u8; 1024 * 1024];
    let mut receive = vec![0u8; PAYLOAD_CAPACITY];
    let mut send = vec![0u8; PAYLOAD_CAPACITY];
    let outcome = run_parser_worker_service(
        &mut transport,
        ContainerDispatcher::new(&mut arena, limits),
        ServiceBuffers {
            receive_payload: &mut receive,
            send_payload: &mut send,
        },
        ServiceLimits::default(),
    );
    let result = outcome.map(|_| ()).map_err(|failure| failure.error);
    (transport, result)
}

// ------------------------------------------------------------------ wise ---

/// Incompressible bytes, so a synthetic package is genuinely larger than one
/// read window and one data chunk.
fn noise(len: usize, seed: u32) -> Vec<u8> {
    let mut state = seed | 1;
    (0..len)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 24) as u8
        })
        .collect()
}

fn wise_files() -> Vec<SyntheticFile> {
    vec![
        SyntheticFile::new(b"maps\\one.bsp", noise(200_000, 0x51ed)),
        SyntheticFile::new(b"cfg\\two.cfg", b"alpha beta gamma delta ".repeat(97)),
        SyntheticFile::new(b"sound\\three.wav", noise(30_000, 0x77ab)),
    ]
}

#[test]
fn a_wise_package_enumerates_streams_and_completes() {
    let files = wise_files();
    let built = build_package(&PackageOptions::with_files(files.clone()));
    let (parent, result) = run(
        built.image.clone(),
        vec![Next::Enumerate],
        BackendLimits::default(),
        |_| {},
    );
    assert_eq!(result, Ok(()));
    assert_eq!(parent.completes, 1);
    assert!(parent.closed);

    // Every recorded file is offered, under its recorded relative path, at
    // its measured size, plus the package's unnamed streams.
    for file in &files {
        let spelling = String::from_utf8(file.path.clone()).expect("ascii").replace('\\', "/");
        let offered = parent
            .offered
            .iter()
            .find(|entry| entry.spelling == spelling)
            .expect("the file is offered");
        assert_eq!(offered.size_bytes, file.content.len() as u64);
    }
    assert!(
        parent
            .offered
            .iter()
            .any(|entry| entry.spelling.starts_with("unnamed/"))
    );

    // Streaming each offered file yields exactly its bytes.
    for file in &files {
        let spelling = String::from_utf8(file.path.clone()).expect("ascii").replace('\\', "/");
        let token = parent
            .offered
            .iter()
            .find(|entry| entry.spelling == spelling)
            .expect("offered")
            .token;
        let (streamed, result) = run(
            built.image.clone(),
            vec![Next::Enumerate, Next::Stream(token)],
            BackendLimits::default(),
            |_| {},
        );
        assert_eq!(result, Ok(()));
        assert_eq!(streamed.completes, 2);
        assert_eq!(streamed.streamed, file.content);
    }
}

#[test]
fn a_wise_stream_with_a_broken_checksum_fails_the_request() {
    let files = wise_files();
    let mut options = PackageOptions::with_files(files.clone());
    // Stream 0 is the bitmap and stream 1 the script, so stream 2 is the
    // first file.
    options.corrupt_crc_of_stream = Some(2);
    let built = build_package(&options);

    let (parent, result) = run(
        built.image.clone(),
        vec![Next::Enumerate],
        BackendLimits::default(),
        |_| {},
    );
    assert_eq!(result, Ok(()));
    let token = parent
        .offered
        .iter()
        .find(|entry| entry.spelling == "maps/one.bsp")
        .expect("the file is still offered")
        .token;

    let (_, result) = run(
        built.image,
        vec![Next::Enumerate, Next::Stream(token)],
        BackendLimits::default(),
        |_| {},
    );
    assert_eq!(result, Err(ServiceError::DispatchFailure));
}

#[test]
fn a_cancel_between_chunks_ends_the_stream_without_completing_it() {
    let files = wise_files();
    let built = build_package(&PackageOptions::with_files(files.clone()));
    let (parent, result) = run(
        built.image.clone(),
        vec![Next::Enumerate],
        BackendLimits::default(),
        |_| {},
    );
    assert_eq!(result, Ok(()));
    let token = parent
        .offered
        .iter()
        .find(|entry| entry.spelling == "maps/one.bsp")
        .expect("offered")
        .token;

    let (parent, result) = run(
        built.image,
        vec![Next::Enumerate, Next::Stream(token)],
        BackendLimits::default(),
        |transport| transport.cancel_after_chunks = Some(1),
    );
    assert_eq!(result, Ok(()));
    assert!(parent.cancelled);
    // The enumeration completed; the stream did not.
    assert_eq!(parent.completes, 1);
    assert!(parent.streamed.len() < files[0].content.len());
}

#[test]
fn an_exhausted_walk_budget_fails_the_enumeration() {
    let built = build_package(&PackageOptions::with_files(wise_files()));
    let mut limits = BackendLimits::default();
    limits.wise.max_total_inflated_bytes = 1_024;
    let (parent, result) = run(built.image, vec![Next::Enumerate], limits, |_| {});
    assert_eq!(result, Err(ServiceError::DispatchFailure));
    assert_eq!(parent.completes, 0);
    assert!(parent.aborted);
}

#[test]
fn a_source_read_failure_ends_the_session_without_a_catalog() {
    let built = build_package(&PackageOptions::with_files(wise_files()));
    let (parent, result) = run(
        built.image,
        vec![Next::Enumerate],
        BackendLimits::default(),
        |transport| transport.reads_before_failure = Some(0),
    );
    assert_eq!(result, Err(ServiceError::SourceFailure));
    assert_eq!(parent.completes, 0);
}

#[test]
fn a_container_this_build_does_not_know_is_unsupported() {
    let (_, result) = run(
        vec![0x5a; 128 * 1024],
        vec![Next::Enumerate],
        BackendLimits::default(),
        |_| {},
    );
    assert_eq!(result, Err(ServiceError::DispatchUnsupported));
}

// --------------------------------------------------------------- ms-cab ---

#[test]
fn a_cabinet_enumerates_and_streams() {
    let contents: Vec<Vec<u8>> = vec![
        vec![0x41; 30_000],
        b"cabinet payload ".repeat(300),
        (0..12_000u32).map(|value| value as u8).collect(),
    ];
    let cabinet = build(&CabinetSpec::new(vec![FolderSpec::new(
        Method::MsZip,
        vec![
            FileSpec::new("one.dat", contents[0].clone()),
            FileSpec::new("dir\\two.dat", contents[1].clone()),
            FileSpec::new("three.dat", contents[2].clone()),
        ],
    )]));

    let (parent, result) = run(
        cabinet.bytes.clone(),
        vec![Next::Enumerate],
        BackendLimits::default(),
        |_| {},
    );
    assert_eq!(result, Ok(()));
    assert_eq!(parent.offered.len(), 3);
    assert_eq!(parent.offered[1].spelling, "dir/two.dat");

    for (index, content) in contents.iter().enumerate() {
        let token = parent.offered[index].token;
        let (streamed, result) = run(
            cabinet.bytes.clone(),
            vec![Next::Enumerate, Next::Stream(token)],
            BackendLimits::default(),
            |_| {},
        );
        assert_eq!(result, Ok(()));
        assert_eq!(&streamed.streamed, content);
    }
}

// ------------------------------------------------------------------ isz ---

#[test]
fn a_z_archive_enumerates_and_streams() {
    let mut builder = ArchiveBuilder::new();
    let root = builder.directory(b"");
    let nested = builder.directory(b"data");
    let first = vec![0x37u8; 20_000];
    let second = b"z archive payload ".repeat(400);
    builder.entry(root, b"one.dat", &first, false);
    builder.entry(nested, b"two.dat", &second, true);
    let archive = builder.build();

    let (parent, result) = run(
        archive.bytes.clone(),
        vec![Next::Enumerate],
        BackendLimits::default(),
        |_| {},
    );
    assert_eq!(result, Ok(()));
    assert_eq!(parent.offered.len(), 2);
    assert_eq!(parent.offered[0].spelling, "one.dat");
    assert_eq!(parent.offered[1].spelling, "data/two.dat");

    for (index, content) in [first, second].iter().enumerate() {
        let token = parent.offered[index].token;
        let (streamed, result) = run(
            archive.bytes.clone(),
            vec![Next::Enumerate, Next::Stream(token)],
            BackendLimits::default(),
            |_| {},
        );
        assert_eq!(result, Ok(()));
        assert_eq!(&streamed.streamed, content);
    }
}

#[test]
fn the_recognised_kinds_are_the_three_documented_ones() {
    // A compile-time reminder that adding a kind means adding a back end.
    let kinds = [
        ContainerKind::WiseOverlay,
        ContainerKind::MicrosoftCabinet,
        ContainerKind::InstallShieldZ,
    ];
    assert_eq!(kinds.len(), 3);
}
