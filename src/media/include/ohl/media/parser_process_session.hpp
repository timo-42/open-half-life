#pragma once

#include "ohl/media/parser_frame_channel.hpp"
#include "ohl/media/parser_parent_handshake.hpp"
#include "ohl/media/parser_parent_session.hpp"
#include "ohl/platform/isolated_worker.hpp"

#include <chrono>
#include <cstddef>
#include <cstdint>
#include <memory>
#include <span>

namespace ohl::media {

// Hands out fresh, nonzero, monotonically increasing protocol session IDs and
// worker epochs. Values are unique for the allocator's lifetime and are never
// reused, including after exhaustion. Deterministic and free of randomness:
// production composition default-constructs starting at 1; the explicit
// constructor exists so tests can drive exhaustion without iterating the
// entire 64-bit space. A zero starting value is treated as already exhausted
// so the allocator never hands out session ID or epoch zero.
enum class ParserSessionIdAllocatorError : std::uint8_t {
  none,
  exhausted,
};

struct ParserSessionAllocation {
  std::uint64_t session_id{0};
  std::uint64_t worker_epoch{0};
};

struct ParserSessionIdAllocationResult {
  ParserSessionIdAllocatorError error{ParserSessionIdAllocatorError::none};
  ParserSessionAllocation allocation;

  [[nodiscard]] bool valid() const noexcept {
    return error == ParserSessionIdAllocatorError::none;
  }
};

class ParserSessionIdAllocator final {
 public:
  ParserSessionIdAllocator() noexcept = default;
  ParserSessionIdAllocator(std::uint64_t first_session_id,
                            std::uint64_t first_worker_epoch) noexcept;

  ParserSessionIdAllocator(const ParserSessionIdAllocator&) = delete;
  ParserSessionIdAllocator& operator=(const ParserSessionIdAllocator&) =
      delete;
  ParserSessionIdAllocator(ParserSessionIdAllocator&&) noexcept = default;
  ParserSessionIdAllocator& operator=(ParserSessionIdAllocator&&) noexcept =
      default;

  // Fails closed (returns exhausted) rather than wrapping once either counter
  // has issued its maximum representable value.
  [[nodiscard]] ParserSessionIdAllocationResult allocate() noexcept;

 private:
  std::uint64_t next_session_id_{1};
  std::uint64_t next_worker_epoch_{1};
  bool exhausted_{false};
};

enum class ParserProcessSessionError : std::uint8_t {
  none,
  invalid_configuration,
  invalid_state,
  handshake_failed,
  session_create_failed,
  shutdown_failed,
};

enum class ParserProcessSessionState : std::uint8_t {
  idle,
  open,
  closed,
  terminated,
};

struct ParserProcessSessionOpenResult {
  ParserProcessSessionError error{ParserProcessSessionError::none};
  ParserParentHandshakeError handshake_error{ParserParentHandshakeError::none};
  ParserParentSessionError session_error{ParserParentSessionError::none};
  platform::IsolatedWorkerWaitResult termination_result;

  [[nodiscard]] bool valid() const noexcept {
    return error == ParserProcessSessionError::none;
  }
};

struct ParserProcessSessionShutdownResult {
  ParserProcessSessionError error{ParserProcessSessionError::none};
  ParserParentSessionError session_error{ParserParentSessionError::none};
  platform::IsolatedWorkerWaitResult wait_result;
  bool escalated{false};

  [[nodiscard]] bool valid() const noexcept {
    return error == ParserProcessSessionError::none;
  }
};

// Disconnected owner of exactly one confined parser worker's process
// lifetime. It owns the launched `platform::IsolatedWorker`, the frame
// channel adapted over its exact I/O, and the `ParserParentSession` built on
// top of a successful handshake. It has no raw-path, destination, cache,
// staging, publication, or component-selection authority: callers drive
// enumerate/stream/cancel/shutdown protocol operations through the exposed
// `ParserParentSession`, and this type is responsible only for keeping the
// channel alive through handshake-proof consumption and session lifetime,
// then closing and reaping the worker in the correct order.
//
// Move-only, non-copyable. The destructor never abandons a live worker: if
// the session has not already reached a terminal state, it escalates to
// `terminate_and_wait()` with a short bounded deadline. That escalation is
// idempotent and cached; it happens at most once regardless of how many
// times shutdown is attempted or the object is destroyed.
class ParserProcessSession final {
 public:
  ParserProcessSession(std::unique_ptr<platform::IsolatedWorker> worker,
                       std::uint64_t session_id,
                       std::uint64_t worker_epoch) noexcept;
  ~ParserProcessSession();

  ParserProcessSession(const ParserProcessSession&) = delete;
  ParserProcessSession& operator=(const ParserProcessSession&) = delete;
  ParserProcessSession(ParserProcessSession&& other) noexcept;
  ParserProcessSession& operator=(ParserProcessSession&& other) noexcept;

  // Performs the parent hello/ready handshake over the borrowed worker's
  // exact I/O, then constructs the `ParserParentSession` from the resulting
  // proof. The frame channel and the handshake receive storage must remain
  // exactly as documented by `perform_parser_parent_handshake`. On any
  // failure the worker is escalated to `terminate_and_wait()` exactly once
  // and this object becomes terminal; open() may not be retried.
  [[nodiscard]] ParserProcessSessionOpenResult open(
      const ValidatedMedia& media, ParserSourceReadLimits source_read_limits,
      parser::ProtocolBudgets protocol_budgets,
      std::span<std::byte> handshake_receive_storage,
      std::chrono::steady_clock::time_point deadline,
      PayloadImportLimits import_limits = {},
      platform::IsolatedWorkerCancellationToken cancellation = {}) noexcept;

  // Performs orderly protocol shutdown through the `ParserParentSession`,
  // closes the worker's channel, then waits for and reaps the worker. Any
  // shutdown failure or reap timeout escalates to `terminate_and_wait()`
  // exactly once instead. Idempotent: once closed or terminated, repeated
  // calls return the cached outcome without further worker interaction.
  [[nodiscard]] ParserProcessSessionShutdownResult orderly_shutdown(
      std::chrono::steady_clock::time_point deadline,
      platform::IsolatedWorkerCancellationToken cancellation = {}) noexcept;

  // Exposes the owned session so a caller can drive enumerate/stream/cancel
  // protocol operations. Returns nullptr before a successful open().
  [[nodiscard]] ParserParentSession* session() noexcept;
  [[nodiscard]] const ParserParentSession* session() const noexcept;

  [[nodiscard]] ParserProcessSessionState state() const noexcept {
    return state_;
  }
  [[nodiscard]] bool terminal() const noexcept {
    return state_ == ParserProcessSessionState::closed ||
           state_ == ParserProcessSessionState::terminated;
  }
  [[nodiscard]] std::uint64_t session_id() const noexcept {
    return session_id_;
  }
  [[nodiscard]] std::uint64_t worker_epoch() const noexcept {
    return worker_epoch_;
  }

 private:
  [[nodiscard]] platform::IsolatedWorkerWaitResult terminate_and_wait_once(
      std::chrono::steady_clock::time_point deadline) noexcept;

  // Declaration order fixes destruction order (reverse): parent_session_ is
  // destroyed first (it may abort the still-live channel_, which targets the
  // still-live worker_), then channel_, then worker_ last.
  std::unique_ptr<platform::IsolatedWorker> worker_;
  std::unique_ptr<ParserFrameChannel> channel_;
  std::unique_ptr<ParserParentSession> parent_session_;

  std::uint64_t session_id_{0};
  std::uint64_t worker_epoch_{0};
  ParserProcessSessionState state_{ParserProcessSessionState::idle};
  bool terminated_once_{false};
  platform::IsolatedWorkerWaitResult cached_termination_;
};

}  // namespace ohl::media
