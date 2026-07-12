#pragma once

#include "ohl/parser/protocol.hpp"
#include "ohl/parser/protocol_messages.hpp"

#include <cstddef>
#include <cstdint>
#include <span>

namespace ohl::parser::detail {

enum class ParserWorkerIoStatus : std::uint8_t {
  ok,
  peer_closed,
  failed,
};

enum class ParserWorkerInputStatus : std::uint8_t {
  unavailable,
  available,
  peer_closed,
  failed,
};

// Non-owning, synchronous exact-I/O transport capability. Successful reads
// and writes transfer the complete non-empty span, and probe_input() must not
// consume bytes. No callback may re-enter the service. A callback must not
// retain a reference, pointer, or view passed by the service; read/write spans
// are valid only for that callback invocation. In particular, callbacks have
// no independent access to the service buffers unless the service supplies a
// span for the current invocation. abort_io() and close_io() are idempotent.
// The service copies this callback table, but context is non-owning; the caller
// keeps the referenced context storage alive for the complete invocation.
struct ParserWorkerTransportOperations {
  using ReadExact = ParserWorkerIoStatus (*)(
      void* context, std::span<std::byte> destination) noexcept;
  using WriteAll = ParserWorkerIoStatus (*)(
      void* context, std::span<const std::byte> source) noexcept;
  using ProbeInput = ParserWorkerInputStatus (*)(void* context) noexcept;
  using EndIo = void (*)(void* context) noexcept;

  ReadExact read_exact{nullptr};
  WriteAll write_all{nullptr};
  ProbeInput probe_input{nullptr};
  EndIo abort_io{nullptr};
  EndIo close_io{nullptr};
  void* context{nullptr};

  [[nodiscard]] bool valid() const noexcept {
    return read_exact != nullptr && write_all != nullptr &&
           probe_input != nullptr && abort_io != nullptr &&
           close_io != nullptr && context != nullptr;
  }
};

enum class ParserWorkerOperation : std::uint8_t {
  enumerate,
  stream,
};

enum class ParserWorkerDispatchStatus : std::uint8_t {
  ok,
  unsupported,
  failed,
};

struct ParserWorkerDispatchBeginResult {
  ParserWorkerDispatchStatus status{ParserWorkerDispatchStatus::failed};
  // Required only for stream. It is the exact number of data bytes that the
  // operation will emit before canonical completion.
  std::uint64_t stream_size{0};
};

enum class ParserWorkerDispatchStepKind : std::uint8_t {
  need_read,
  entry_batch,
  data_chunk,
  complete,
};

struct ParserWorkerDispatchStep {
  ParserWorkerDispatchStepKind kind{ParserWorkerDispatchStepKind::complete};
  // need_read
  std::uint64_t read_offset{0};
  std::uint32_t read_length{0};
  // entry_batch
  std::span<const EntryBatchEntry> entries;
  // data_chunk
  std::span<const std::byte> data;
};

// The dispatcher is trusted implementation code but owns no transport or
// source capability. begin(), step(), accept_read_reply(), cancel(), and end()
// are synchronous, allocation-free, noexcept, and must not re-enter the
// service or retain any reference or view passed by the service. Each step must
// perform bounded work and return exactly one action. Views returned through a
// step output remain alive and immutable until the service next invokes a
// transport or dispatcher callback. The service fully validates and encodes
// those views into send_payload before that next callback; it never
// dereferences a returned view afterward. The callback table is copied, but
// context and returned views remain non-owning. The service assigns read
// sequence numbers and validates every action before placing it on the wire.
struct ParserWorkerDispatchOperations {
  using Begin = ParserWorkerDispatchBeginResult (*)(
      void* context, ParserWorkerOperation operation,
      std::uint64_t source_token,
      const SourceReadPolicy& source_policy) noexcept;
  using Step = ParserWorkerDispatchStatus (*)(
      void* context, ParserWorkerDispatchStep& output) noexcept;
  using AcceptReadReply = ParserWorkerDispatchStatus (*)(
      void* context, const ReadReplyMessage& reply) noexcept;
  using End = void (*)(void* context) noexcept;

  Begin begin{nullptr};
  Step step{nullptr};
  AcceptReadReply accept_read_reply{nullptr};
  End cancel{nullptr};
  End end{nullptr};
  void* context{nullptr};

  [[nodiscard]] bool valid() const noexcept {
    return begin != nullptr && step != nullptr &&
           accept_read_reply != nullptr && cancel != nullptr && end != nullptr &&
           context != nullptr;
  }
};

struct ParserWorkerServiceBuffers {
  // Non-owning, caller-provided scratch storage. Both backing allocations must
  // hold the protocol payload maximum and be non-null. Their ranges must be
  // disjoint from each other, both operation tables, and all callback context
  // and operation storage reachable through those tables. They remain alive
  // exclusively for the complete service invocation. The service may overwrite
  // either buffer after the callback consuming its current view returns;
  // callbacks must neither retain those views nor access the backing storage
  // independently.
  std::span<std::byte> receive_payload;
  std::span<std::byte> send_payload;
};

inline constexpr std::uint64_t kMaximumParserWorkerDispatchSteps =
    kMaximumProtocolMessages;

struct ParserWorkerServiceLimits {
  ProtocolBudgets protocol_budgets;
  std::uint64_t maximum_dispatch_steps{kMaximumParserWorkerDispatchSteps};

  [[nodiscard]] bool valid() const noexcept {
    return protocol_budgets.valid() && maximum_dispatch_steps != 0 &&
           maximum_dispatch_steps <= kMaximumParserWorkerDispatchSteps;
  }
};

enum class ParserWorkerServiceError : std::uint8_t {
  none,
  invalid_configuration,
  transport_failure,
  protocol_failure,
  dispatch_unsupported,
  dispatch_failure,
  source_failure,
  dispatch_budget_exceeded,
  internal_failure,
};

struct ParserWorkerServiceResult {
  ParserWorkerServiceError error{ParserWorkerServiceError::none};
  ProtocolError protocol_error{ProtocolError::none};
  ParserWorkerIoStatus io_status{ParserWorkerIoStatus::ok};
  ParserWorkerDispatchStatus dispatch_status{ParserWorkerDispatchStatus::ok};
  std::uint64_t session_id{0};
  std::uint64_t dispatch_steps{0};
  bool clean_shutdown{false};

  [[nodiscard]] bool valid() const noexcept {
    return error == ParserWorkerServiceError::none && clean_shutdown;
  }
};

// Runs one complete worker lifetime. Internal unsupported is terminal and
// emits no frame. Invalid configuration is rejected before read, write, or
// probe I/O. After valid configuration establishes the transport, every
// non-clean return aborts it; canonical shutdown closes it. Operation tables
// are copied; buffer spans and callback contexts are non-owning and their
// backing storage remains caller-owned.
[[nodiscard]] ParserWorkerServiceResult run_parser_worker_service(
    ParserWorkerTransportOperations transport,
    ParserWorkerDispatchOperations dispatch,
    ParserWorkerServiceBuffers buffers,
    ParserWorkerServiceLimits limits = {}) noexcept;

}  // namespace ohl::parser::detail
