#define OHL_LINUX_ISOLATED_WORKER_FREESTANDING 1
#include "isolated_worker_linux_internal.hpp"

#include "parser_worker_service_internal.hpp"

#include <asm/unistd.h>
#include <poll.h>
#include <signal.h>
#include <sys/socket.h>

#include <array>
#include <cstddef>
#include <cstdint>
#include <span>

extern "C" std::size_t strlen(const char* const value) noexcept {
  std::size_t size = 0;
  while (value[size] != '\0') {
    ++size;
  }
  return size;
}

extern "C" void* memmove(void* const destination, const void* const source,
                          const std::size_t size) noexcept {
  auto* const output = static_cast<unsigned char*>(destination);
  const auto* const input = static_cast<const unsigned char*>(source);
  const auto output_address = reinterpret_cast<std::uintptr_t>(output);
  const auto input_address = reinterpret_cast<std::uintptr_t>(input);
  if (output_address < input_address) {
    for (std::size_t index = 0; index < size; ++index) {
      output[index] = input[index];
    }
  } else if (output_address > input_address) {
    for (std::size_t index = size; index != 0U; --index) {
      output[index - 1U] = input[index - 1U];
    }
  }
  return destination;
}

namespace {

using ohl::platform::detail::linux_isolated_worker::kWorkerChannelDescriptor;
using ohl::platform::detail::linux_isolated_worker::kWorkerReadyAttestation;
using ohl::platform::detail::linux_isolated_worker::kWorkerReadyDescriptor;
namespace parser = ohl::parser;
namespace worker = ohl::parser::detail;

constexpr long kInterrupted = -4;
constexpr long kTryAgain = -11;
constexpr int kCleanExit = 0;
constexpr int kProtocolErrorExit = 64;
constexpr int kUnsupportedExit = 65;
constexpr int kTransportErrorExit = 66;
constexpr int kInternalErrorExit = 70;

[[nodiscard]] long raw_syscall(const long number, const long argument1 = 0,
                               const long argument2 = 0,
                               const long argument3 = 0,
                               const long argument4 = 0,
                               const long argument5 = 0,
                               const long argument6 = 0) noexcept {
  long result = 0;
  register long register10 __asm__("r10") = argument4;
  register long register8 __asm__("r8") = argument5;
  register long register9 __asm__("r9") = argument6;
  __asm__ volatile("syscall"
                   : "=a"(result)
                   : "a"(number), "D"(argument1), "S"(argument2),
                     "d"(argument3), "r"(register10), "r"(register8),
                     "r"(register9)
                   : "rcx", "r11", "memory");
  return result;
}

[[noreturn]] void exit_worker(const int status) noexcept {
  static_cast<void>(raw_syscall(__NR_exit_group, status));
  __builtin_unreachable();
}

struct KernelSignalAction {
  std::uintptr_t handler{0U};
  unsigned long flags{0U};
  std::uintptr_t restorer{0U};
  unsigned long mask{0U};
};

[[nodiscard]] bool ignore_broken_pipe() noexcept {
  KernelSignalAction action{.handler = 1U};
  return raw_syscall(__NR_rt_sigaction, SIGPIPE,
                     reinterpret_cast<long>(&action), 0,
                     static_cast<long>(sizeof(action.mask))) == 0;
}

[[nodiscard]] bool wait_for(const int descriptor, const short events) noexcept {
  struct pollfd item{descriptor, events, 0};
  while (true) {
    const long result =
        raw_syscall(__NR_poll, reinterpret_cast<long>(&item), 1, -1);
    if (result > 0) {
      return true;
    }
    if (result != kInterrupted) {
      return false;
    }
  }
}

[[nodiscard]] worker::ParserWorkerIoStatus read_exact(
    void*, const std::span<std::byte> destination) noexcept {
  std::size_t offset = 0;
  while (offset < destination.size()) {
    const long amount = raw_syscall(__NR_read, kWorkerChannelDescriptor,
        reinterpret_cast<long>(destination.data() +
                               static_cast<std::ptrdiff_t>(offset)),
        static_cast<long>(destination.size() - offset));
    if (amount > 0) {
      offset += static_cast<std::size_t>(amount);
      continue;
    }
    if (amount == 0) {
      return offset == 0U ? worker::ParserWorkerIoStatus::peer_closed
                          : worker::ParserWorkerIoStatus::failed;
    }
    if ((amount == kInterrupted || amount == kTryAgain) &&
        wait_for(kWorkerChannelDescriptor, POLLIN)) {
      continue;
    }
    return worker::ParserWorkerIoStatus::failed;
  }
  return worker::ParserWorkerIoStatus::ok;
}

[[nodiscard]] worker::ParserWorkerIoStatus write_all(
    void*, const std::span<const std::byte> source) noexcept {
  std::size_t offset = 0;
  while (offset < source.size()) {
    const long amount = raw_syscall(__NR_sendto, kWorkerChannelDescriptor,
        reinterpret_cast<long>(source.data() +
                               static_cast<std::ptrdiff_t>(offset)),
        static_cast<long>(source.size() - offset), MSG_NOSIGNAL, 0, 0);
    if (amount > 0) {
      offset += static_cast<std::size_t>(amount);
      continue;
    }
    if ((amount == kInterrupted || amount == kTryAgain) &&
        wait_for(kWorkerChannelDescriptor, POLLOUT)) {
      continue;
    }
    return worker::ParserWorkerIoStatus::failed;
  }
  return worker::ParserWorkerIoStatus::ok;
}

[[nodiscard]] worker::ParserWorkerInputStatus probe_input(void*) noexcept {
  std::byte byte{};
  while (true) {
    const long amount = raw_syscall(
        __NR_recvfrom, kWorkerChannelDescriptor, reinterpret_cast<long>(&byte),
        1, MSG_PEEK | MSG_DONTWAIT, 0, 0);
    if (amount > 0) {
      return worker::ParserWorkerInputStatus::available;
    }
    if (amount == 0) {
      return worker::ParserWorkerInputStatus::peer_closed;
    }
    if (amount == kInterrupted) {
      continue;
    }
    if (amount == kTryAgain) {
      return worker::ParserWorkerInputStatus::unavailable;
    }
    return worker::ParserWorkerInputStatus::failed;
  }
}

struct TransportState {
  bool ended{false};
};

void end_io(void* const context) noexcept {
  auto& state = *static_cast<TransportState*>(context);
  if (!state.ended) {
    state.ended = true;
    static_cast<void>(raw_syscall(__NR_close, kWorkerChannelDescriptor));
  }
}

struct UnsupportedDispatcher {};

[[nodiscard]] worker::ParserWorkerDispatchBeginResult begin_dispatch(
    void*, worker::ParserWorkerOperation, std::uint64_t,
    const parser::SourceReadPolicy&) noexcept {
  return {.status = worker::ParserWorkerDispatchStatus::unsupported};
}

[[nodiscard]] worker::ParserWorkerDispatchStatus step_dispatch(
    void*, worker::ParserWorkerDispatchStep&) noexcept {
  return worker::ParserWorkerDispatchStatus::unsupported;
}

[[nodiscard]] worker::ParserWorkerDispatchStatus accept_read_reply(
    void*, const parser::ReadReplyMessage&) noexcept {
  return worker::ParserWorkerDispatchStatus::unsupported;
}

void end_dispatch(void*) noexcept {}

alignas(4096) std::array<std::byte, parser::kMaximumFramePayloadBytes>
    receive_payload;
alignas(4096) std::array<std::byte, parser::kMaximumFramePayloadBytes>
    send_payload;
TransportState transport_state;
UnsupportedDispatcher dispatcher_state;

[[nodiscard]] bool emit_readiness_attestation() noexcept {
  std::size_t offset = 0;
  while (offset < kWorkerReadyAttestation.size()) {
    const long amount = raw_syscall(
        __NR_write, kWorkerReadyDescriptor,
        reinterpret_cast<long>(kWorkerReadyAttestation.data() +
                               static_cast<std::ptrdiff_t>(offset)),
        static_cast<long>(kWorkerReadyAttestation.size() - offset));
    if (amount > 0) {
      offset += static_cast<std::size_t>(amount);
      continue;
    }
    if ((amount == kInterrupted || amount == kTryAgain) &&
        wait_for(kWorkerReadyDescriptor, POLLOUT)) {
      continue;
    }
    return false;
  }
  static_cast<void>(raw_syscall(__NR_close, kWorkerReadyDescriptor));
  return true;
}

[[nodiscard]] int sanitized_exit_status(
    const worker::ParserWorkerServiceResult& result) noexcept {
  if (result.valid() ||
      (result.error == worker::ParserWorkerServiceError::transport_failure &&
       result.io_status == worker::ParserWorkerIoStatus::peer_closed)) {
    return kCleanExit;
  }
  switch (result.error) {
    case worker::ParserWorkerServiceError::protocol_failure:
      return kProtocolErrorExit;
    case worker::ParserWorkerServiceError::dispatch_unsupported:
      return kUnsupportedExit;
    case worker::ParserWorkerServiceError::transport_failure:
      return kTransportErrorExit;
    case worker::ParserWorkerServiceError::none:
      return kCleanExit;
    case worker::ParserWorkerServiceError::invalid_configuration:
    case worker::ParserWorkerServiceError::dispatch_failure:
    case worker::ParserWorkerServiceError::source_failure:
    case worker::ParserWorkerServiceError::dispatch_budget_exceeded:
    case worker::ParserWorkerServiceError::internal_failure:
      return kInternalErrorExit;
  }
  return kInternalErrorExit;
}

[[noreturn]] void serve_parser_protocol() noexcept {
  if (!ignore_broken_pipe()) {
    exit_worker(kInternalErrorExit);
  }
  if (!emit_readiness_attestation()) {
    exit_worker(kTransportErrorExit);
  }
  const worker::ParserWorkerTransportOperations transport{
      .read_exact = read_exact,
      .write_all = write_all,
      .probe_input = probe_input,
      .abort_io = end_io,
      .close_io = end_io,
      .context = &transport_state,
  };
  const worker::ParserWorkerDispatchOperations dispatcher{
      .begin = begin_dispatch,
      .step = step_dispatch,
      .accept_read_reply = accept_read_reply,
      .cancel = end_dispatch,
      .end = end_dispatch,
      .context = &dispatcher_state,
  };
  const auto result = worker::run_parser_worker_service(
      transport, dispatcher,
      {.receive_payload = receive_payload, .send_payload = send_payload});
  exit_worker(sanitized_exit_status(result));
}

}  // namespace

extern "C" [[noreturn]] void ohl_media_parser_worker_main() noexcept {
  serve_parser_protocol();
}

__asm__(R"(
  .global _start
  .type _start,@function
_start:
  and $-16, %rsp
  call ohl_media_parser_worker_main
  ud2
  .size _start, .-_start
)");
