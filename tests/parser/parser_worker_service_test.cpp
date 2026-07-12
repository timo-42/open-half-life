#include "parser_worker_service_internal.hpp"

#include <algorithm>
#include <array>
#include <cstddef>
#include <cstdint>
#include <cstdlib>
#include <iostream>
#include <span>
#include <string>
#include <string_view>
#include <vector>

namespace {

namespace parser = ohl::parser;
namespace worker = ohl::parser::detail;

constexpr std::uint64_t kSessionId = 0x1020304050607080ULL;
constexpr std::uint64_t kSourceSize = 4096U;
constexpr std::uint32_t kMaximumReadBytes = 256U;

[[noreturn]] void fail(const std::string_view message) {
  std::cerr << "parser worker service test failed: " << message << '\n';
  std::exit(1);
}

void require(const bool condition, const std::string_view message) {
  if (!condition) {
    fail(message);
  }
}

void append_frame(std::vector<std::byte>& bytes,
                  const parser::FrameHeader& header,
                  const std::span<const std::byte> payload = {}) {
  std::vector<std::byte> frame(parser::kFrameHeaderBytes + payload.size());
  const auto encoded = parser::encode_frame(header, payload, frame);
  require(encoded.valid() && encoded.bytes_written == frame.size(),
          "test frame encoding failed");
  bytes.insert(bytes.end(), frame.begin(), frame.end());
}

void append_header(std::vector<std::byte>& bytes,
                   const parser::FrameHeader& header) {
  std::array<std::byte, parser::kFrameHeaderBytes> encoded{};
  require(parser::encode_frame_header(header, encoded) ==
              parser::ProtocolError::none,
          "test header encoding failed");
  bytes.insert(bytes.end(), encoded.begin(), encoded.end());
}

void store_u32_little_endian(const std::span<std::byte> bytes,
                             const std::size_t offset,
                             const std::uint32_t value) {
  require(offset <= bytes.size() && sizeof(value) <= bytes.size() - offset,
          "test little-endian write is out of range");
  for (std::size_t index = 0U; index < sizeof(value); ++index) {
    bytes[offset + index] = static_cast<std::byte>(
        (value >> static_cast<unsigned int>(index * 8U)) & 0xffU);
  }
}

void append_hello(std::vector<std::byte>& bytes) {
  std::array<std::byte, parser::kHelloPayloadBytes> payload{};
  const auto encoded = parser::encode_hello_payload(
      {.source_size = kSourceSize,
       .maximum_read_bytes = kMaximumReadBytes},
      payload);
  require(encoded.valid(), "hello encoding failed");
  append_frame(bytes,
               {.type = parser::MessageType::hello,
                .payload_length = static_cast<std::uint32_t>(payload.size()),
                .session_id = kSessionId,
                .request_id = 0U},
               payload);
}

void append_shutdown(std::vector<std::byte>& bytes) {
  append_frame(bytes,
               {.type = parser::MessageType::shutdown,
                .payload_length = 0U,
                .session_id = kSessionId,
                .request_id = 0U});
}

void append_enumerate(std::vector<std::byte>& bytes,
                      const std::uint64_t request_id = 1U) {
  append_frame(bytes,
               {.type = parser::MessageType::enumerate,
                .payload_length = 0U,
                .session_id = kSessionId,
                .request_id = request_id});
}

void append_stream(std::vector<std::byte>& bytes,
                   const std::uint64_t request_id,
                   const std::uint64_t source_token) {
  std::array<std::byte, parser::kStreamEntryPayloadBytes> payload{};
  const auto encoded = parser::encode_stream_entry_payload(
      {.source_token = source_token}, payload);
  require(encoded.valid(), "stream request encoding failed");
  append_frame(bytes,
               {.type = parser::MessageType::stream_entry,
                .payload_length = static_cast<std::uint32_t>(payload.size()),
                .session_id = kSessionId,
                .request_id = request_id},
               payload);
}

void append_cancel(std::vector<std::byte>& bytes,
                   const std::uint64_t request_id = 1U) {
  append_frame(bytes,
               {.type = parser::MessageType::cancel,
                .payload_length = 0U,
                .session_id = kSessionId,
                .request_id = request_id});
}

void append_read_reply(std::vector<std::byte>& bytes,
                       const std::uint64_t request_id,
                       const std::uint32_t sequence,
                       const std::span<const std::byte> data,
                       const parser::ProtocolStatus status =
                           parser::ProtocolStatus::ok,
                       const std::uint32_t requested_length = 0U) {
  std::vector<std::byte> payload(parser::kReadReplyPrefixBytes + data.size());
  const auto encoded = parser::encode_read_reply_payload(
      {.read_sequence = sequence, .status = status, .data = data}, sequence,
      requested_length == 0U ? static_cast<std::uint32_t>(data.size())
                             : requested_length,
      payload);
  require(encoded.valid(), "read reply encoding failed");
  append_frame(bytes,
               {.type = parser::MessageType::read_reply,
                .payload_length =
                    static_cast<std::uint32_t>(encoded.bytes_written),
                .session_id = kSessionId,
                .request_id = request_id},
               std::span<const std::byte>{payload}.first(encoded.bytes_written));
}

struct ScriptedTransport {
  std::vector<std::byte> input;
  std::vector<std::byte> output;
  std::vector<worker::ParserWorkerInputStatus> probes;
  std::size_t input_position{0U};
  std::size_t probe_position{0U};
  std::size_t read_calls{0U};
  std::size_t write_calls{0U};
  std::size_t probe_calls{0U};
  std::size_t abort_calls{0U};
  std::size_t close_calls{0U};
  std::size_t fail_read_call{0U};
  std::size_t fail_write_call{0U};
  worker::ParserWorkerIoStatus read_failure{
      worker::ParserWorkerIoStatus::peer_closed};
  worker::ParserWorkerIoStatus write_failure{
      worker::ParserWorkerIoStatus::failed};
  parser::EntryBatchEntry* mutate_entry_on_write{nullptr};
  std::size_t mutation_write_call{0U};
  std::uint64_t replacement_source_token{0U};
  bool aborted{false};
  bool closed{false};

  ScriptedTransport() { output.reserve(4U * 1024U * 1024U); }
};

worker::ParserWorkerIoStatus read_exact(
    void* context, const std::span<std::byte> destination) noexcept {
  auto& transport = *static_cast<ScriptedTransport*>(context);
  ++transport.read_calls;
  if (transport.fail_read_call == transport.read_calls) {
    return transport.read_failure;
  }
  if (destination.size() > transport.input.size() -
                               std::min(transport.input_position,
                                        transport.input.size())) {
    return worker::ParserWorkerIoStatus::peer_closed;
  }
  std::copy_n(transport.input.data() + transport.input_position,
              destination.size(), destination.data());
  transport.input_position += destination.size();
  return worker::ParserWorkerIoStatus::ok;
}

worker::ParserWorkerIoStatus write_all(
    void* context, const std::span<const std::byte> source) noexcept {
  auto& transport = *static_cast<ScriptedTransport*>(context);
  ++transport.write_calls;
  if (transport.fail_write_call == transport.write_calls) {
    return transport.write_failure;
  }
  transport.output.insert(transport.output.end(), source.begin(), source.end());
  if (transport.mutation_write_call == transport.write_calls &&
      transport.mutate_entry_on_write != nullptr) {
    transport.mutate_entry_on_write->source_token =
        transport.replacement_source_token;
  }
  return worker::ParserWorkerIoStatus::ok;
}

worker::ParserWorkerInputStatus probe_input(void* context) noexcept {
  auto& transport = *static_cast<ScriptedTransport*>(context);
  ++transport.probe_calls;
  if (transport.probe_position < transport.probes.size()) {
    return transport.probes[transport.probe_position++];
  }
  return worker::ParserWorkerInputStatus::unavailable;
}

void abort_io(void* context) noexcept {
  auto& transport = *static_cast<ScriptedTransport*>(context);
  ++transport.abort_calls;
  transport.aborted = true;
}

void close_io(void* context) noexcept {
  auto& transport = *static_cast<ScriptedTransport*>(context);
  ++transport.close_calls;
  transport.closed = true;
}

worker::ParserWorkerTransportOperations transport_operations(
    ScriptedTransport& transport) {
  return {
      .read_exact = read_exact,
      .write_all = write_all,
      .probe_input = probe_input,
      .abort_io = abort_io,
      .close_io = close_io,
      .context = &transport,
  };
}

struct ScriptedDispatch {
  worker::ParserWorkerDispatchBeginResult begin_result{
      .status = worker::ParserWorkerDispatchStatus::ok,
      .stream_size = 0U};
  std::vector<worker::ParserWorkerDispatchStep> steps;
  std::size_t step_position{0U};
  std::size_t begin_calls{0U};
  std::size_t step_calls{0U};
  std::size_t accept_calls{0U};
  std::size_t cancel_calls{0U};
  std::size_t end_calls{0U};
  worker::ParserWorkerDispatchStatus step_status{
      worker::ParserWorkerDispatchStatus::ok};
  worker::ParserWorkerDispatchStatus accept_status{
      worker::ParserWorkerDispatchStatus::ok};
  worker::ParserWorkerOperation operation{
      worker::ParserWorkerOperation::enumerate};
  std::uint64_t source_token{0U};
  parser::SourceReadPolicy source_policy{};
  std::array<std::byte, kMaximumReadBytes> accepted_read{};
  std::size_t accepted_read_size{0U};
  std::uint32_t accepted_read_sequence{0U};
  bool began{false};
  bool cancelled{false};
  bool ended{false};
};

worker::ParserWorkerDispatchStep complete_step() {
  worker::ParserWorkerDispatchStep step;
  step.kind = worker::ParserWorkerDispatchStepKind::complete;
  return step;
}

worker::ParserWorkerDispatchStep read_step(const std::uint64_t offset,
                                           const std::uint32_t length) {
  worker::ParserWorkerDispatchStep step;
  step.kind = worker::ParserWorkerDispatchStepKind::need_read;
  step.read_offset = offset;
  step.read_length = length;
  return step;
}

worker::ParserWorkerDispatchStep entry_step(
    const std::span<const parser::EntryBatchEntry> entries) {
  worker::ParserWorkerDispatchStep step;
  step.kind = worker::ParserWorkerDispatchStepKind::entry_batch;
  step.entries = entries;
  return step;
}

worker::ParserWorkerDispatchStep data_step(
    const std::span<const std::byte> data) {
  worker::ParserWorkerDispatchStep step;
  step.kind = worker::ParserWorkerDispatchStepKind::data_chunk;
  step.data = data;
  return step;
}

worker::ParserWorkerDispatchBeginResult begin_dispatch(
    void* context, const worker::ParserWorkerOperation operation,
    const std::uint64_t source_token,
    const parser::SourceReadPolicy& source_policy) noexcept {
  auto& dispatch = *static_cast<ScriptedDispatch*>(context);
  ++dispatch.begin_calls;
  dispatch.began = true;
  dispatch.operation = operation;
  dispatch.source_token = source_token;
  dispatch.source_policy = source_policy;
  return dispatch.begin_result;
}

worker::ParserWorkerDispatchStatus step_dispatch(
    void* context, worker::ParserWorkerDispatchStep& output) noexcept {
  auto& dispatch = *static_cast<ScriptedDispatch*>(context);
  ++dispatch.step_calls;
  if (dispatch.step_status != worker::ParserWorkerDispatchStatus::ok) {
    return dispatch.step_status;
  }
  if (dispatch.step_position >= dispatch.steps.size()) {
    return worker::ParserWorkerDispatchStatus::failed;
  }
  output = dispatch.steps[dispatch.step_position++];
  return worker::ParserWorkerDispatchStatus::ok;
}

worker::ParserWorkerDispatchStatus accept_read_reply(
    void* context, const parser::ReadReplyMessage& reply) noexcept {
  auto& dispatch = *static_cast<ScriptedDispatch*>(context);
  ++dispatch.accept_calls;
  dispatch.accepted_read_sequence = reply.read_sequence;
  dispatch.accepted_read_size = reply.data.size();
  if (reply.data.size() <= dispatch.accepted_read.size()) {
    std::copy(reply.data.begin(), reply.data.end(),
              dispatch.accepted_read.begin());
  }
  return dispatch.accept_status;
}

void cancel_dispatch(void* context) noexcept {
  auto& dispatch = *static_cast<ScriptedDispatch*>(context);
  ++dispatch.cancel_calls;
  dispatch.cancelled = true;
}

void end_dispatch(void* context) noexcept {
  auto& dispatch = *static_cast<ScriptedDispatch*>(context);
  ++dispatch.end_calls;
  dispatch.ended = true;
}

worker::ParserWorkerDispatchOperations dispatch_operations(
    ScriptedDispatch& dispatch) {
  return {
      .begin = begin_dispatch,
      .step = step_dispatch,
      .accept_read_reply = accept_read_reply,
      .cancel = cancel_dispatch,
      .end = end_dispatch,
      .context = &dispatch,
  };
}

std::vector<parser::FrameView> decode_output(
    const std::vector<std::byte>& bytes) {
  std::vector<parser::FrameView> frames;
  std::size_t position = 0U;
  while (position < bytes.size()) {
    require(bytes.size() - position >= parser::kFrameHeaderBytes,
            "truncated output header");
    const auto header = parser::decode_frame_header(
        std::span<const std::byte, parser::kFrameHeaderBytes>{
            bytes.data() + position, parser::kFrameHeaderBytes});
    require(header.valid(), "invalid output header");
    const auto size = parser::kFrameHeaderBytes + header.header.payload_length;
    require(size <= bytes.size() - position, "truncated output frame");
    const auto frame = parser::decode_frame(
        std::span<const std::byte>{bytes.data() + position, size}, kSessionId);
    require(frame.valid(), "invalid output frame");
    frames.push_back(frame);
    position += size;
  }
  return frames;
}

worker::ParserWorkerServiceResult run_service_with_buffers(
    ScriptedTransport& transport, ScriptedDispatch& dispatch,
    const worker::ParserWorkerServiceBuffers buffers,
    const worker::ParserWorkerServiceLimits limits = {},
    worker::ParserWorkerTransportOperations transport_ops = {},
    worker::ParserWorkerDispatchOperations dispatch_ops = {}) {
  if (transport_ops.context == nullptr) {
    transport_ops = transport_operations(transport);
  }
  if (dispatch_ops.context == nullptr) {
    dispatch_ops = dispatch_operations(dispatch);
  }
  return worker::run_parser_worker_service(
      transport_ops, dispatch_ops, buffers, limits);
}

worker::ParserWorkerServiceResult run_service(
    ScriptedTransport& transport, ScriptedDispatch& dispatch,
    const worker::ParserWorkerServiceLimits limits = {}) {
  std::vector<std::byte> receive(parser::kMaximumFramePayloadBytes);
  std::vector<std::byte> send(parser::kMaximumFramePayloadBytes);
  return run_service_with_buffers(
      transport, dispatch,
      {.receive_payload = receive, .send_payload = send}, limits);
}

void require_invalid_configuration_rejected(
    const worker::ParserWorkerServiceBuffers buffers,
    const worker::ParserWorkerServiceLimits limits = {},
    worker::ParserWorkerTransportOperations transport_ops = {},
    worker::ParserWorkerDispatchOperations dispatch_ops = {}) {
  ScriptedTransport transport;
  ScriptedDispatch dispatch;
  if (transport_ops.context == nullptr) {
    transport_ops = transport_operations(transport);
  }
  if (dispatch_ops.context == nullptr) {
    dispatch_ops = dispatch_operations(dispatch);
  }

  const auto result = run_service_with_buffers(
      transport, dispatch, buffers, limits, transport_ops, dispatch_ops);
  require(result.error ==
                  worker::ParserWorkerServiceError::invalid_configuration &&
              result.session_id == 0U && !result.clean_shutdown,
          "invalid service configuration was accepted");
  require(transport.read_calls == 0U && transport.write_calls == 0U &&
              transport.probe_calls == 0U && transport.abort_calls == 1U &&
              transport.close_calls == 0U && transport.output.empty(),
          "invalid configuration performed transport I/O or wrong cleanup");
  require(dispatch.begin_calls == 0U && dispatch.step_calls == 0U &&
              dispatch.accept_calls == 0U && dispatch.cancel_calls == 0U &&
              dispatch.end_calls == 0U,
          "invalid configuration invoked the dispatcher");
}

void require_terminal_dispatch_failure(
    const worker::ParserWorkerServiceResult& result,
    const ScriptedTransport& transport, const ScriptedDispatch& dispatch,
    const std::string_view message) {
  require(result.error == worker::ParserWorkerServiceError::dispatch_failure &&
              !result.clean_shutdown && transport.abort_calls == 1U &&
              transport.close_calls == 0U && dispatch.cancel_calls == 1U &&
              dispatch.end_calls == 0U,
          message);
}

void test_invalid_configuration_is_pre_io_and_fail_closed() {
  std::vector<std::byte> receive(parser::kMaximumFramePayloadBytes);
  std::vector<std::byte> send(parser::kMaximumFramePayloadBytes);
  std::vector<std::byte> short_buffer(parser::kMaximumFramePayloadBytes - 1U);
  std::vector<std::byte> overlapping(parser::kMaximumFramePayloadBytes + 1U);

  require_invalid_configuration_rejected(
      {.receive_payload = {}, .send_payload = send});
  require_invalid_configuration_rejected(
      {.receive_payload = receive, .send_payload = {}});
  require_invalid_configuration_rejected(
      {.receive_payload = short_buffer, .send_payload = send});
  require_invalid_configuration_rejected(
      {.receive_payload = receive, .send_payload = short_buffer});
  require_invalid_configuration_rejected(
      {.receive_payload = overlapping,
       .send_payload = std::span<std::byte>{overlapping}.subspan(1U)});

  require_invalid_configuration_rejected(
      {.receive_payload = receive, .send_payload = send},
      {.protocol_budgets = {}, .maximum_dispatch_steps = 0U});
  require_invalid_configuration_rejected(
      {.receive_payload = receive, .send_payload = send},
      {.protocol_budgets = {},
       .maximum_dispatch_steps =
           worker::kMaximumParserWorkerDispatchSteps + 1U});
  require_invalid_configuration_rejected(
      {.receive_payload = receive, .send_payload = send},
      {.protocol_budgets = {.maximum_messages = 1U,
                            .maximum_payload_bytes =
                                parser::kMaximumCumulativePayloadBytes},
       .maximum_dispatch_steps = 1U});
  require_invalid_configuration_rejected(
      {.receive_payload = receive, .send_payload = send},
      {.protocol_budgets = {
           .maximum_messages = 2U,
           .maximum_payload_bytes = parser::kHelloPayloadBytes - 1U},
       .maximum_dispatch_steps = 1U});

}

void test_dispatcher_views_overlapping_service_buffers_are_rejected() {
  {
    ScriptedTransport transport;
    append_hello(transport.input);
    append_stream(transport.input, 1U, 1U);
    ScriptedDispatch dispatch;
    std::vector<std::byte> receive(parser::kMaximumFramePayloadBytes);
    std::vector<std::byte> send(parser::kMaximumFramePayloadBytes);
    dispatch.begin_result.stream_size = 1U;
    dispatch.steps = {data_step(std::span<const std::byte>{send}.first(1U))};

    const auto result = run_service_with_buffers(
        transport, dispatch,
        {.receive_payload = receive, .send_payload = send});
    require_terminal_dispatch_failure(
        result, transport, dispatch,
        "send-buffer-backed data view was not rejected exactly once");
    require(result.protocol_error == parser::ProtocolError::noncanonical_value &&
                decode_output(transport.output).size() == 1U,
            "send-buffer overlap emitted an operation frame");
  }
  {
    ScriptedTransport transport;
    append_hello(transport.input);
    append_enumerate(transport.input);
    ScriptedDispatch dispatch;
    std::vector<std::byte> receive(parser::kMaximumFramePayloadBytes);
    std::vector<std::byte> send(parser::kMaximumFramePayloadBytes);
    const std::array entry{parser::EntryBatchEntry{
        .source_token = 1U,
        .size_bytes = 0U,
        .archive_path = std::string_view{
            reinterpret_cast<const char*>(receive.data()), 1U}}};
    dispatch.steps = {entry_step(entry)};

    const auto result = run_service_with_buffers(
        transport, dispatch,
        {.receive_payload = receive, .send_payload = send});
    require_terminal_dispatch_failure(
        result, transport, dispatch,
        "receive-buffer-backed path view was not rejected exactly once");
    require(result.protocol_error == parser::ProtocolError::noncanonical_value &&
                decode_output(transport.output).size() == 1U,
            "path-buffer overlap emitted an operation frame");
  }
}

void test_invalid_begin_sizes_and_stream_completion_are_fail_closed() {
  {
    ScriptedTransport transport;
    append_hello(transport.input);
    append_enumerate(transport.input);
    ScriptedDispatch dispatch;
    dispatch.begin_result.stream_size = 1U;

    const auto result = run_service(transport, dispatch);
    require_terminal_dispatch_failure(
        result, transport, dispatch,
        "enumeration stream size was not rejected exactly once");
    require(dispatch.begin_calls == 1U && dispatch.step_calls == 0U &&
                decode_output(transport.output).size() == 1U,
            "invalid enumeration begin emitted an operation frame");
  }
  {
    ScriptedTransport transport;
    append_hello(transport.input);
    append_stream(transport.input, 1U, 1U);
    ScriptedDispatch dispatch;
    dispatch.begin_result.stream_size =
        parser::kMaximumEnumeratedEntryBytes + 1U;

    const auto result = run_service(transport, dispatch);
    require_terminal_dispatch_failure(
        result, transport, dispatch,
        "over-limit stream size was not rejected exactly once");
    require(dispatch.begin_calls == 1U && dispatch.step_calls == 0U &&
                decode_output(transport.output).size() == 1U,
            "invalid stream begin emitted an operation frame");
  }
  {
    ScriptedTransport transport;
    append_hello(transport.input);
    append_stream(transport.input, 1U, 1U);
    const std::array data{std::byte{1}, std::byte{2}, std::byte{3}};
    ScriptedDispatch dispatch;
    dispatch.begin_result.stream_size = 2U;
    dispatch.steps = {data_step(data)};

    const auto result = run_service(transport, dispatch);
    require_terminal_dispatch_failure(
        result, transport, dispatch,
        "stream over-completion was not rejected exactly once");
    require(decode_output(transport.output).size() == 1U,
            "over-completion emitted a data frame");
  }
  {
    ScriptedTransport transport;
    append_hello(transport.input);
    append_stream(transport.input, 1U, 1U);
    const std::array data{std::byte{1}};
    ScriptedDispatch dispatch;
    dispatch.begin_result.stream_size = 2U;
    dispatch.steps = {data_step(data), complete_step()};

    const auto result = run_service(transport, dispatch);
    require_terminal_dispatch_failure(
        result, transport, dispatch,
        "stream under-completion was not rejected exactly once");
    const auto frames = decode_output(transport.output);
    require(frames.size() == 2U &&
                frames[1].header.type == parser::MessageType::data_chunk,
            "under-completion emitted a completion frame");
  }
}

void test_probe_failures_abort_without_dispatch_step() {
  constexpr std::array cases{
      worker::ParserWorkerInputStatus::peer_closed,
      worker::ParserWorkerInputStatus::failed,
      static_cast<worker::ParserWorkerInputStatus>(0xffU),
  };
  for (const auto status : cases) {
    ScriptedTransport transport;
    append_hello(transport.input);
    append_enumerate(transport.input);
    transport.probes = {status};
    ScriptedDispatch dispatch;
    dispatch.steps = {complete_step()};

    const auto result = run_service(transport, dispatch);
    const auto expected_error =
        status == static_cast<worker::ParserWorkerInputStatus>(0xffU)
            ? worker::ParserWorkerServiceError::internal_failure
            : worker::ParserWorkerServiceError::transport_failure;
    require(result.error == expected_error && dispatch.begin_calls == 1U &&
                dispatch.step_calls == 0U && dispatch.cancel_calls == 1U &&
                dispatch.end_calls == 0U && transport.probe_calls == 1U &&
                transport.abort_calls == 1U && transport.close_calls == 0U,
            "probe failure did not abort before dispatch exactly once");
    require(decode_output(transport.output).size() == 1U,
            "probe failure emitted an operation frame");
  }
}

void test_payload_write_failure_cancels_and_aborts_once() {
  ScriptedTransport transport;
  append_hello(transport.input);
  append_enumerate(transport.input);
  transport.fail_write_call = 3U;
  const std::array entry{parser::EntryBatchEntry{
      .source_token = 1U, .size_bytes = 1U, .archive_path = "x"}};
  ScriptedDispatch dispatch;
  dispatch.steps = {entry_step(entry)};

  const auto result = run_service(transport, dispatch);
  require(result.error == worker::ParserWorkerServiceError::transport_failure &&
              result.io_status == worker::ParserWorkerIoStatus::failed &&
              dispatch.begin_calls == 1U && dispatch.step_calls == 1U &&
              dispatch.cancel_calls == 1U && dispatch.end_calls == 0U &&
              transport.write_calls == 3U && transport.abort_calls == 1U &&
              transport.close_calls == 0U,
          "payload write failure did not clean up exactly once");
  require(transport.output.size() == parser::kFrameHeaderBytes * 2U,
          "payload write failure emitted unexpected payload bytes");
}

void test_invalid_headers_do_not_consume_payload_or_dispatch() {
  {
    ScriptedTransport transport;
    append_hello(transport.input);
    const auto invalid_frame_offset = transport.input.size();
    append_header(transport.input,
                  {.type = parser::MessageType::enumerate,
                   .payload_length = 1U,
                   .session_id = kSessionId + 1U,
                   .request_id = 1U});
    transport.input.push_back(std::byte{0x5a});
    ScriptedDispatch dispatch;

    const auto result = run_service(transport, dispatch);
    require(result.error == worker::ParserWorkerServiceError::protocol_failure &&
                result.protocol_error == parser::ProtocolError::wrong_session_id &&
                transport.read_calls == 3U &&
                transport.input_position ==
                    invalid_frame_offset + parser::kFrameHeaderBytes &&
                dispatch.begin_calls == 0U && dispatch.step_calls == 0U &&
                transport.abort_calls == 1U && transport.close_calls == 0U,
            "wrong-session header consumed payload or invoked dispatch");
    require(decode_output(transport.output).size() == 1U,
            "wrong-session header emitted an operation frame");
  }
  {
    ScriptedTransport transport;
    append_hello(transport.input);
    const auto invalid_frame_offset = transport.input.size();
    append_header(transport.input,
                  {.type = parser::MessageType::enumerate,
                   .payload_length = 0U,
                   .session_id = kSessionId,
                   .request_id = 1U});
    store_u32_little_endian(
        transport.input, invalid_frame_offset + 12U,
        parser::kMaximumFramePayloadBytes + 1U);
    transport.input.push_back(std::byte{0x5a});
    ScriptedDispatch dispatch;

    const auto result = run_service(transport, dispatch);
    require(result.error == worker::ParserWorkerServiceError::protocol_failure &&
                result.protocol_error == parser::ProtocolError::payload_too_large &&
                transport.read_calls == 3U &&
                transport.input_position ==
                    invalid_frame_offset + parser::kFrameHeaderBytes &&
                dispatch.begin_calls == 0U && dispatch.step_calls == 0U &&
                transport.abort_calls == 1U && transport.close_calls == 0U,
            "oversized header consumed payload or invoked dispatch");
    require(decode_output(transport.output).size() == 1U,
            "oversized header emitted an operation frame");
  }
}

void test_handshake_and_shutdown() {
  ScriptedTransport transport;
  append_hello(transport.input);
  append_shutdown(transport.input);
  ScriptedDispatch dispatch;

  const auto result = run_service(transport, dispatch);
  require(result.valid() && result.session_id == kSessionId,
          "canonical service lifetime failed");
  require(transport.closed && !transport.aborted &&
              transport.close_calls == 1U && transport.abort_calls == 0U,
          "canonical shutdown did not close cleanly");
  require(!dispatch.began, "shutdown invoked dispatch");
  const auto frames = decode_output(transport.output);
  require(frames.size() == 1U &&
              frames[0].header.type == parser::MessageType::ready &&
              parser::decode_ready_payload(frames[0]).valid(),
          "canonical ready frame is wrong");
}

void test_successful_enumeration() {
  ScriptedTransport transport;
  append_hello(transport.input);
  append_enumerate(transport.input);
  append_shutdown(transport.input);

  const std::array entries{
      parser::EntryBatchEntry{.source_token = 2U,
                              .size_bytes = 3U,
                              .archive_path = "a.bin"},
      parser::EntryBatchEntry{.source_token = 4U,
                              .size_bytes = 5U,
                              .archive_path = "b.bin"},
  };
  ScriptedDispatch dispatch;
  dispatch.steps = {
      entry_step(entries),
      complete_step(),
  };

  const auto result = run_service(transport, dispatch);
  require(result.valid() && result.dispatch_steps == 2U,
          "enumeration service lifetime failed");
  require(dispatch.began && dispatch.ended && !dispatch.cancelled &&
              dispatch.begin_calls == 1U && dispatch.end_calls == 1U &&
              dispatch.cancel_calls == 0U &&
              dispatch.operation == worker::ParserWorkerOperation::enumerate &&
              dispatch.source_policy.source_size == kSourceSize,
          "enumeration dispatch binding is wrong");

  const auto frames = decode_output(transport.output);
  require(frames.size() == 3U &&
              frames[1].header.type == parser::MessageType::entry_batch &&
              frames[2].header.type == parser::MessageType::complete,
          "enumeration output sequence is wrong");
  std::array<parser::EntryBatchEntry, 2> decoded_entries{};
  const auto batch = parser::decode_entry_batch_payload(
      frames[1],
      {.remaining_entries = parser::kMaximumEnumeratedEntries,
       .remaining_path_bytes = parser::kMaximumEnumeratedPathBytes,
       .maximum_entry_bytes = parser::kMaximumEnumeratedEntryBytes,
       .remaining_total_bytes = parser::kMaximumEnumeratedTotalBytes},
      decoded_entries);
  require(batch.valid() && batch.message.entries.size() == entries.size(),
          "enumeration batch is not canonical");
  require(parser::decode_complete_payload(frames[2],
                                          parser::ProtocolPhase::enumerate)
              .valid(),
          "enumeration completion is not canonical");
}

void test_entry_batch_count_and_path_boundaries() {
  {
    ScriptedTransport transport;
    append_hello(transport.input);
    append_enumerate(transport.input);
    append_shutdown(transport.input);
    std::vector<parser::EntryBatchEntry> entries(
        parser::kMaximumEntryBatchEntries);
    for (std::size_t index = 0U; index < entries.size(); ++index) {
      entries[index] = {
          .source_token = index + 1U,
          .size_bytes = 0U,
          .archive_path = "x",
      };
    }
    ScriptedDispatch dispatch;
    dispatch.steps = {entry_step(entries), complete_step()};

    const auto result = run_service(transport, dispatch);
    require(result.valid() && dispatch.ended,
            "exact entry-count boundary was rejected");
  }
  {
    ScriptedTransport transport;
    append_hello(transport.input);
    append_enumerate(transport.input);
    std::vector<parser::EntryBatchEntry> entries(
        static_cast<std::size_t>(parser::kMaximumEntryBatchEntries) + 1U);
    for (std::size_t index = 0U; index < entries.size(); ++index) {
      entries[index] = {
          .source_token = index + 1U,
          .size_bytes = 0U,
          .archive_path = "x",
      };
    }
    ScriptedDispatch dispatch;
    dispatch.steps = {entry_step(entries)};

    const auto result = run_service(transport, dispatch);
    require(result.error == worker::ParserWorkerServiceError::dispatch_failure &&
                result.protocol_error ==
                    parser::ProtocolError::noncanonical_value &&
                dispatch.cancelled && transport.aborted,
            "over-limit entry count was accepted");
  }
  {
    ScriptedTransport transport;
    append_hello(transport.input);
    append_enumerate(transport.input);
    append_shutdown(transport.input);
    const std::string path(
        static_cast<std::size_t>(parser::kMaximumEntryBatchPathBytes), 'p');
    const std::array entries{parser::EntryBatchEntry{
        .source_token = 1U, .size_bytes = 0U, .archive_path = path}};
    ScriptedDispatch dispatch;
    dispatch.steps = {entry_step(entries), complete_step()};

    const auto result = run_service(transport, dispatch);
    require(result.valid() && dispatch.ended,
            "exact entry-path boundary was rejected");
  }
  {
    ScriptedTransport transport;
    append_hello(transport.input);
    append_enumerate(transport.input);
    const std::string path(
        static_cast<std::size_t>(parser::kMaximumEntryBatchPathBytes) + 1U,
        'p');
    const std::array entries{parser::EntryBatchEntry{
        .source_token = 1U, .size_bytes = 0U, .archive_path = path}};
    ScriptedDispatch dispatch;
    dispatch.steps = {entry_step(entries)};

    const auto result = run_service(transport, dispatch);
    require(result.error == worker::ParserWorkerServiceError::dispatch_failure &&
                result.protocol_error ==
                    parser::ProtocolError::noncanonical_value &&
                dispatch.cancelled && transport.aborted,
            "over-limit entry path was accepted");
  }
}

void test_dispatch_views_are_not_used_after_transport_callback() {
  ScriptedTransport transport;
  append_hello(transport.input);
  append_enumerate(transport.input);
  append_shutdown(transport.input);

  std::array first_entries{parser::EntryBatchEntry{
      .source_token = 1U, .size_bytes = 0U, .archive_path = "first"}};
  const std::array second_entries{parser::EntryBatchEntry{
      .source_token = 2U, .size_bytes = 0U, .archive_path = "second"}};
  transport.mutate_entry_on_write = &first_entries[0];
  transport.mutation_write_call = 2U;
  transport.replacement_source_token = 100U;

  ScriptedDispatch dispatch;
  dispatch.steps = {
      entry_step(first_entries),
      entry_step(second_entries),
      complete_step(),
  };

  const auto result = run_service(transport, dispatch);
  require(result.valid() && dispatch.ended &&
              first_entries[0].source_token ==
                  transport.replacement_source_token,
          "transport-time dispatcher-view mutation affected service state");
  const auto frames = decode_output(transport.output);
  require(frames.size() == 4U &&
              frames[1].header.type == parser::MessageType::entry_batch &&
              frames[2].header.type == parser::MessageType::entry_batch &&
              frames[3].header.type == parser::MessageType::complete,
          "post-write dispatcher-view lifetime output is wrong");
}

void test_successful_stream() {
  ScriptedTransport transport;
  append_hello(transport.input);
  append_stream(transport.input, 1U, 9U);
  append_shutdown(transport.input);

  const std::array data{std::byte{0x11}, std::byte{0x22}, std::byte{0x33}};
  ScriptedDispatch dispatch;
  dispatch.begin_result.stream_size = data.size();
  dispatch.steps = {
      data_step(data),
      complete_step(),
  };

  const auto result = run_service(transport, dispatch);
  require(result.valid() && dispatch.ended && dispatch.end_calls == 1U &&
              dispatch.cancel_calls == 0U &&
              dispatch.operation == worker::ParserWorkerOperation::stream &&
              dispatch.source_token == 9U,
          "stream service lifetime failed");
  const auto frames = decode_output(transport.output);
  require(frames.size() == 3U &&
              frames[1].header.type == parser::MessageType::data_chunk &&
              frames[2].header.type == parser::MessageType::complete,
          "stream output sequence is wrong");
  const auto chunk = parser::decode_data_chunk_payload(frames[1], data.size());
  require(chunk.valid() && std::ranges::equal(chunk.message.data, data),
          "stream data is wrong");
  require(parser::decode_complete_payload(frames[2],
                                          parser::ProtocolPhase::stream)
              .valid(),
          "stream completion is not canonical");
}

void test_unsupported_is_terminal_without_operation_frame() {
  ScriptedTransport transport;
  append_hello(transport.input);
  append_enumerate(transport.input);
  ScriptedDispatch dispatch;
  dispatch.begin_result.status =
      worker::ParserWorkerDispatchStatus::unsupported;

  const auto result = run_service(transport, dispatch);
  require(result.error == worker::ParserWorkerServiceError::dispatch_unsupported &&
              result.dispatch_status ==
                  worker::ParserWorkerDispatchStatus::unsupported &&
              transport.aborted && !transport.closed,
          "unsupported dispatch did not fail terminally");
  const auto frames = decode_output(transport.output);
  require(frames.size() == 1U &&
              frames[0].header.type == parser::MessageType::ready,
          "unsupported dispatch emitted an operation frame");
}

void test_step_unsupported_is_terminal_without_operation_frame() {
  ScriptedTransport transport;
  append_hello(transport.input);
  append_enumerate(transport.input);
  append_shutdown(transport.input);
  ScriptedDispatch dispatch;
  dispatch.step_status = worker::ParserWorkerDispatchStatus::unsupported;

  const auto result = run_service(transport, dispatch);
  require(result.error == worker::ParserWorkerServiceError::dispatch_unsupported &&
              result.protocol_error == parser::ProtocolError::none &&
              result.io_status == worker::ParserWorkerIoStatus::ok &&
              result.dispatch_status ==
                  worker::ParserWorkerDispatchStatus::unsupported &&
              result.session_id == kSessionId && result.dispatch_steps == 1U &&
              !result.clean_shutdown,
          "unsupported dispatch step reported the wrong service result");
  require(dispatch.began && dispatch.cancelled && !dispatch.ended,
          "unsupported dispatch step performed the wrong cleanup");
  require(transport.aborted && !transport.closed,
          "unsupported dispatch step did not abort the transport");
  const auto frames = decode_output(transport.output);
  require(frames.size() == 1U &&
              frames[0].header.type == parser::MessageType::ready,
          "unsupported dispatch step emitted an operation frame");
}

void test_accept_read_reply_unsupported_is_terminal_without_operation_frame() {
  ScriptedTransport transport;
  append_hello(transport.input);
  append_enumerate(transport.input);
  const std::array read_data{std::byte{0x42}, std::byte{0x43}};
  append_read_reply(transport.input, 1U, 1U, read_data);
  append_shutdown(transport.input);
  ScriptedDispatch dispatch;
  dispatch.steps = {
      read_step(10U, static_cast<std::uint32_t>(read_data.size())),
  };
  dispatch.accept_status = worker::ParserWorkerDispatchStatus::unsupported;

  const auto result = run_service(transport, dispatch);
  require(result.error == worker::ParserWorkerServiceError::dispatch_unsupported &&
              result.protocol_error == parser::ProtocolError::none &&
              result.io_status == worker::ParserWorkerIoStatus::ok &&
              result.dispatch_status ==
                  worker::ParserWorkerDispatchStatus::unsupported &&
              result.session_id == kSessionId && result.dispatch_steps == 1U &&
              !result.clean_shutdown,
          "unsupported read acceptance reported the wrong service result");
  require(dispatch.began && dispatch.accepted_read_sequence == 1U &&
              dispatch.accepted_read_size == read_data.size() &&
              dispatch.cancelled && !dispatch.ended,
          "unsupported read acceptance performed the wrong dispatch cleanup");
  require(transport.aborted && !transport.closed,
          "unsupported read acceptance did not abort the transport");
  const auto frames = decode_output(transport.output);
  require(frames.size() == 2U &&
              frames[0].header.type == parser::MessageType::ready &&
              frames[1].header.type == parser::MessageType::read_request,
          "unsupported read acceptance emitted an operation-result frame");
}

void test_transport_and_malformed_failures() {
  {
    ScriptedTransport transport;
    append_hello(transport.input);
    transport.input[0] = std::byte{0};
    ScriptedDispatch dispatch;
    const auto result = run_service(transport, dispatch);
    require(result.error == worker::ParserWorkerServiceError::protocol_failure &&
                result.protocol_error == parser::ProtocolError::invalid_magic &&
                transport.aborted && transport.output.empty(),
            "malformed hello did not fail closed");
  }
  {
    ScriptedTransport transport;
    append_hello(transport.input);
    transport.fail_read_call = 2U;
    ScriptedDispatch dispatch;
    const auto result = run_service(transport, dispatch);
    require(result.error == worker::ParserWorkerServiceError::transport_failure &&
                result.io_status == worker::ParserWorkerIoStatus::peer_closed &&
                transport.aborted,
            "short hello payload did not fail closed");
  }
  {
    ScriptedTransport transport;
    append_hello(transport.input);
    transport.fail_write_call = 1U;
    ScriptedDispatch dispatch;
    const auto result = run_service(transport, dispatch);
    require(result.error == worker::ParserWorkerServiceError::transport_failure &&
                result.io_status == worker::ParserWorkerIoStatus::failed &&
                transport.aborted,
            "ready write failure did not fail closed");
  }
}

void test_source_read_sequence() {
  ScriptedTransport transport;
  append_hello(transport.input);
  append_enumerate(transport.input);
  const std::array read_data{std::byte{0x42}, std::byte{0x43}};
  append_read_reply(transport.input, 1U, 1U, read_data);
  append_shutdown(transport.input);

  ScriptedDispatch dispatch;
  dispatch.steps = {
      read_step(10U, static_cast<std::uint32_t>(read_data.size())),
      complete_step(),
  };
  const auto result = run_service(transport, dispatch);
  require(result.valid() && dispatch.accepted_read_sequence == 1U &&
              dispatch.accepted_read_size == read_data.size() &&
              std::equal(read_data.begin(), read_data.end(),
                         dispatch.accepted_read.begin()),
          "source reply was not delivered exactly once");
  const auto frames = decode_output(transport.output);
  require(frames.size() == 3U &&
              frames[1].header.type == parser::MessageType::read_request,
          "source read request is missing");
  const auto request = parser::decode_read_request_payload(
      frames[1], {.source_size = kSourceSize,
                  .maximum_read_bytes = kMaximumReadBytes},
      1U);
  require(request.valid() && request.message.offset == 10U &&
              request.message.length == read_data.size(),
          "source read request is not canonical");
}

void test_read_reply_status_handling() {
  constexpr std::array failure_statuses{
      parser::ProtocolStatus::source_changed,
      parser::ProtocolStatus::source_read_failed,
  };
  for (const auto status : failure_statuses) {
    ScriptedTransport transport;
    append_hello(transport.input);
    append_enumerate(transport.input);
    append_read_reply(transport.input, 1U, 1U, {}, status, 1U);
    ScriptedDispatch dispatch;
    dispatch.steps = {read_step(0U, 1U)};

    const auto result = run_service(transport, dispatch);
    require(result.error == worker::ParserWorkerServiceError::source_failure &&
                dispatch.accepted_read_sequence == 0U &&
                dispatch.cancelled && transport.aborted,
            "active source failure did not remain terminal");
  }

  constexpr std::array late_statuses{
      parser::ProtocolStatus::ok,
      parser::ProtocolStatus::source_changed,
      parser::ProtocolStatus::source_read_failed,
  };
  const std::array ok_data{std::byte{0x5a}};
  for (const auto status : late_statuses) {
    ScriptedTransport transport;
    append_hello(transport.input);
    append_enumerate(transport.input);
    append_cancel(transport.input);
    const auto reply_data =
        status == parser::ProtocolStatus::ok
            ? std::span<const std::byte>{ok_data}
            : std::span<const std::byte>{};
    append_read_reply(transport.input, 1U, 1U, reply_data, status, 1U);
    append_shutdown(transport.input);
    ScriptedDispatch dispatch;
    dispatch.steps = {read_step(0U, 1U)};

    const auto result = run_service(transport, dispatch);
    require(result.valid() && dispatch.cancelled && !dispatch.ended &&
                dispatch.accepted_read_sequence == 0U && transport.closed &&
                !transport.aborted,
            "canonical post-cancel reply status was not drained");
    const auto frames = decode_output(transport.output);
    require(frames.size() == 3U &&
                frames[1].header.type == parser::MessageType::read_request &&
                frames[2].header.type == parser::MessageType::cancel_ack,
            "late reply drain emitted an unexpected frame");
  }
  {
    ScriptedTransport transport;
    append_hello(transport.input);
    append_enumerate(transport.input);
    append_cancel(transport.input);
    append_read_reply(transport.input, 1U, 2U, ok_data,  // wrong sequence
                      parser::ProtocolStatus::ok, 1U);
    ScriptedDispatch dispatch;
    dispatch.steps = {read_step(0U, 1U)};

    const auto result = run_service(transport, dispatch);
    require(result.error == worker::ParserWorkerServiceError::protocol_failure &&
                result.protocol_error ==
                    parser::ProtocolError::noncanonical_value &&
                dispatch.accepted_read_sequence == 0U && transport.aborted,
            "noncanonical late reply bypassed payload validation");
  }
  {
    ScriptedTransport transport;
    append_hello(transport.input);
    append_enumerate(transport.input);
    append_cancel(transport.input);
    append_read_reply(transport.input, 2U, 1U, ok_data,  // wrong request
                      parser::ProtocolStatus::ok, 1U);
    ScriptedDispatch dispatch;
    dispatch.steps = {read_step(0U, 1U)};

    const auto result = run_service(transport, dispatch);
    require(result.error == worker::ParserWorkerServiceError::protocol_failure &&
                result.protocol_error == parser::ProtocolError::wrong_request_id &&
                dispatch.accepted_read_sequence == 0U && transport.aborted,
            "canonical late reply bypassed protocol observation");
  }
}

void test_cancel_crossings() {
  {
    ScriptedTransport transport;
    append_hello(transport.input);
    append_enumerate(transport.input);
    append_cancel(transport.input);
    append_shutdown(transport.input);
    transport.probes = {worker::ParserWorkerInputStatus::available};
    ScriptedDispatch dispatch;
    dispatch.steps = {complete_step()};
    const auto result = run_service(transport, dispatch);
    require(result.valid() && dispatch.cancelled && !dispatch.ended,
            "pre-step cancellation failed");
    const auto frames = decode_output(transport.output);
    require(frames.size() == 2U &&
                frames[1].header.type == parser::MessageType::cancel_ack,
            "pre-step cancellation acknowledgement is wrong");
  }
  {
    ScriptedTransport transport;
    append_hello(transport.input);
    append_enumerate(transport.input);
    append_cancel(transport.input);
    append_shutdown(transport.input);
    transport.probes = {worker::ParserWorkerInputStatus::unavailable};
    ScriptedDispatch dispatch;
    dispatch.steps = {read_step(0U, 1U)};
    const auto result = run_service(transport, dispatch);
    require(result.valid() && dispatch.cancelled,
            "read/cancel crossing failed");
    const auto frames = decode_output(transport.output);
    require(frames.size() == 3U &&
                frames[1].header.type == parser::MessageType::read_request &&
                frames[2].header.type == parser::MessageType::cancel_ack,
            "read/cancel crossing output is wrong");
  }
  {
    ScriptedTransport transport;
    append_hello(transport.input);
    append_enumerate(transport.input);
    append_cancel(transport.input);
    append_shutdown(transport.input);
    ScriptedDispatch dispatch;
    dispatch.steps = {complete_step()};
    const auto result = run_service(transport, dispatch);
    require(result.valid() && dispatch.ended && !dispatch.cancelled,
            "completion/late-cancel crossing failed");
    const auto frames = decode_output(transport.output);
    require(frames.size() == 2U &&
                frames[1].header.type == parser::MessageType::complete,
            "stale cancel incorrectly produced an acknowledgement");
  }
}

void test_invalid_dispatch_and_budgets() {
  {
    ScriptedTransport transport;
    append_hello(transport.input);
    append_stream(transport.input, 1U, 3U);
    ScriptedDispatch dispatch;
    dispatch.begin_result.stream_size = 1U;
    dispatch.steps = {entry_step({})};
    const auto result = run_service(transport, dispatch);
    require(result.error == worker::ParserWorkerServiceError::dispatch_failure &&
                dispatch.cancelled && transport.aborted,
            "wrong-operation dispatch action was accepted");
  }
  {
    ScriptedTransport transport;
    append_hello(transport.input);
    append_enumerate(transport.input);
    ScriptedDispatch dispatch;
    dispatch.steps = {
        entry_step({}),
        complete_step(),
    };
    const auto result = run_service(
        transport, dispatch,
        {.protocol_budgets = {}, .maximum_dispatch_steps = 1U});
    require(result.error ==
                worker::ParserWorkerServiceError::dispatch_failure,
            "empty entry batch did not fail before the step boundary");
  }
  {
    ScriptedTransport transport;
    append_hello(transport.input);
    append_enumerate(transport.input);
    const std::array entry{parser::EntryBatchEntry{
        .source_token = 1U, .size_bytes = 1U, .archive_path = "x"}};
    ScriptedDispatch dispatch;
    dispatch.steps = {
        entry_step(entry),
        complete_step(),
    };
    const auto result = run_service(
        transport, dispatch,
        {.protocol_budgets = {}, .maximum_dispatch_steps = 1U});
    require(result.error ==
                worker::ParserWorkerServiceError::dispatch_budget_exceeded &&
                result.dispatch_steps == 1U && transport.aborted,
            "dispatch step budget boundary failed");
  }
  {
    ScriptedTransport transport;
    append_hello(transport.input);
    append_shutdown(transport.input);
    ScriptedDispatch dispatch;
    const auto result = run_service(
        transport, dispatch,
        {.protocol_budgets = {.maximum_messages = 3U,
                              .maximum_payload_bytes =
                                  parser::kHelloPayloadBytes},
         .maximum_dispatch_steps = 1U});
    require(result.valid(), "exact handshake/shutdown budget was rejected");
  }
  {
    ScriptedTransport transport;
    append_hello(transport.input);
    append_shutdown(transport.input);
    ScriptedDispatch dispatch;
    const auto result = run_service(
        transport, dispatch,
        {.protocol_budgets = {.maximum_messages = 2U,
                              .maximum_payload_bytes =
                                  parser::kHelloPayloadBytes},
         .maximum_dispatch_steps = 1U});
    require(result.error == worker::ParserWorkerServiceError::protocol_failure &&
                result.protocol_error ==
                    parser::ProtocolError::message_budget_exceeded &&
                transport.aborted,
            "message budget exhaustion did not fail closed");
  }
}

}  // namespace

int main() {
  test_invalid_configuration_is_pre_io_and_fail_closed();
  test_dispatcher_views_overlapping_service_buffers_are_rejected();
  test_invalid_begin_sizes_and_stream_completion_are_fail_closed();
  test_probe_failures_abort_without_dispatch_step();
  test_payload_write_failure_cancels_and_aborts_once();
  test_invalid_headers_do_not_consume_payload_or_dispatch();
  test_handshake_and_shutdown();
  test_successful_enumeration();
  test_entry_batch_count_and_path_boundaries();
  test_dispatch_views_are_not_used_after_transport_callback();
  test_successful_stream();
  test_unsupported_is_terminal_without_operation_frame();
  test_step_unsupported_is_terminal_without_operation_frame();
  test_accept_read_reply_unsupported_is_terminal_without_operation_frame();
  test_transport_and_malformed_failures();
  test_source_read_sequence();
  test_read_reply_status_handling();
  test_cancel_crossings();
  test_invalid_dispatch_and_budgets();
  return 0;
}
