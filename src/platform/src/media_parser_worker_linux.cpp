#define OHL_LINUX_ISOLATED_WORKER_FREESTANDING 1
#include "isolated_worker_linux_internal.hpp"

#include <asm/unistd.h>
#include <poll.h>

#include <cstddef>

namespace {

using ohl::platform::detail::linux_isolated_worker::kWorkerChannelDescriptor;
using ohl::platform::detail::linux_isolated_worker::kWorkerReadyAttestation;
using ohl::platform::detail::linux_isolated_worker::kWorkerReadyDescriptor;

constexpr long kInterrupted = -4;
constexpr long kTryAgain = -11;

[[nodiscard]] long raw_syscall(const long number, const long argument1 = 0,
                               const long argument2 = 0,
                               const long argument3 = 0) noexcept {
  long result = 0;
  __asm__ volatile("syscall"
                   : "=a"(result)
                   : "a"(number), "D"(argument1), "S"(argument2),
                     "d"(argument3)
                   : "rcx", "r11", "memory");
  return result;
}

[[noreturn]] void exit_worker(const int status) noexcept {
  static_cast<void>(raw_syscall(__NR_exit_group, status));
  __builtin_unreachable();
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

[[nodiscard]] bool write_all(const int descriptor, const std::byte* data,
                             const std::size_t size) noexcept {
  std::size_t offset = 0;
  while (offset < size) {
    const long amount = raw_syscall(
        __NR_write, descriptor,
        reinterpret_cast<long>(data + static_cast<std::ptrdiff_t>(offset)),
        static_cast<long>(size - offset));
    if (amount > 0) {
      offset += static_cast<std::size_t>(amount);
      continue;
    }
    if ((amount == kInterrupted || amount == kTryAgain) &&
        wait_for(descriptor, POLLOUT)) {
      continue;
    }
    return false;
  }
  return true;
}

[[noreturn]] void serve_lifecycle_only() noexcept {
  if (!write_all(kWorkerReadyDescriptor, kWorkerReadyAttestation.data(),
                 kWorkerReadyAttestation.size())) {
    exit_worker(90);
  }
  static_cast<void>(raw_syscall(__NR_close, kWorkerReadyDescriptor));

  std::byte buffer[256]{};
  while (true) {
    const long amount = raw_syscall(
        __NR_read, kWorkerChannelDescriptor, reinterpret_cast<long>(buffer),
        sizeof(buffer));
    if (amount == 0) {
      exit_worker(0);
    }
    if (amount > 0) {
      continue;
    }
    if ((amount == kInterrupted || amount == kTryAgain) &&
        wait_for(kWorkerChannelDescriptor, POLLIN)) {
      continue;
    }
    exit_worker(91);
  }
}

}  // namespace

extern "C" [[noreturn]] void ohl_media_parser_worker_main() noexcept {
  serve_lifecycle_only();
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
