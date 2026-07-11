#include "isolated_worker_linux_internal.hpp"

#include <asm/unistd.h>
#include <fcntl.h>
#include <poll.h>
#include <sys/resource.h>
#include <sys/stat.h>

#include <cstddef>
#include <cstdint>

#ifndef OHL_LINUX_TEST_WORKER_MODE
#define OHL_LINUX_TEST_WORKER_MODE 0
#endif

namespace {

using ohl::platform::detail::linux_isolated_worker::kWorkerChannelDescriptor;
using ohl::platform::detail::linux_isolated_worker::kWorkerReadyAttestation;
using ohl::platform::detail::linux_isolated_worker::kWorkerReadyDescriptor;

constexpr long kBadFileDescriptor = -9;
constexpr long kInterrupted = -4;
constexpr long kTryAgain = -11;
constexpr rlim_t kAddressSpaceLimit = 512U * 1024U * 1024U;
constexpr rlim_t kDataLimit = 256U * 1024U * 1024U;
constexpr rlim_t kStackLimit = 8U * 1024U * 1024U;

[[nodiscard]] long raw_syscall(const long number, const long argument1 = 0,
                               const long argument2 = 0,
                               const long argument3 = 0,
                               const long argument4 = 0,
                               const long argument5 = 0,
                               const long argument6 = 0) noexcept {
  register long r10 __asm__("r10") = argument4;
  register long r8 __asm__("r8") = argument5;
  register long r9 __asm__("r9") = argument6;
  long result = 0;
  __asm__ volatile("syscall"
                   : "=a"(result)
                   : "a"(number), "D"(argument1), "S"(argument2),
                     "d"(argument3), "r"(r10), "r"(r8), "r"(r9)
                   : "rcx", "r11", "memory");
  return result;
}

[[noreturn]] void exit_worker(const int status) noexcept {
  static_cast<void>(raw_syscall(__NR_exit_group, status));
  __builtin_unreachable();
}

[[nodiscard]] bool write_all(const int descriptor, const std::byte* data,
                             const std::size_t size) noexcept {
  std::size_t offset = 0;
  while (offset < size) {
    const long amount = raw_syscall(
        __NR_write, descriptor,
        reinterpret_cast<long>(data + static_cast<std::ptrdiff_t>(offset)),
        static_cast<long>(size - offset));
    if (amount <= 0) {
      if (amount == kInterrupted || amount == kTryAgain) {
        struct pollfd output{descriptor, POLLOUT, 0};
        static_cast<void>(raw_syscall(
            __NR_poll, reinterpret_cast<long>(&output), 1, -1));
        continue;
      }
      return false;
    }
    offset += static_cast<std::size_t>(amount);
  }
  return true;
}

[[nodiscard]] bool equal_string(const char* value,
                                const char* expected) noexcept {
  std::size_t index = 0;
  while (value[index] == expected[index] && expected[index] != '\0') {
    ++index;
  }
  return value[index] == expected[index];
}

[[nodiscard]] bool descriptor_inventory_is_exact() noexcept {
  struct stat status {};
  for (int descriptor = 0; descriptor <= 6; ++descriptor) {
    const long result = raw_syscall(
        __NR_fstat, descriptor, reinterpret_cast<long>(&status));
    const bool expected_open = descriptor == kWorkerChannelDescriptor ||
                               descriptor == kWorkerReadyDescriptor;
    if ((expected_open && result != 0) ||
        (!expected_open && result != kBadFileDescriptor)) {
      return false;
    }
  }
  return true;
}

[[nodiscard]] bool limit_matches(const int resource, const rlim_t value) noexcept {
  struct rlimit limit {};
  return raw_syscall(__NR_prlimit64, 0, resource, 0,
                     reinterpret_cast<long>(&limit)) == 0 &&
         limit.rlim_cur == value && limit.rlim_max == value;
}

[[nodiscard]] bool runtime_inventory_is_exact(
    const std::uintptr_t* initial_stack) noexcept {
  const auto argument_count = initial_stack[0];
  if (argument_count != 1U) {
    return false;
  }
  const auto* argument =
      reinterpret_cast<const char*>(initial_stack[1]);
  if (!equal_string(argument, "ohl-media-parser-worker")) {
    return false;
  }
  const auto environment_start = argument_count + 2U;
  if (initial_stack[environment_start] != 0U) {
    return false;
  }

  char working_directory[4]{};
  if (raw_syscall(__NR_getcwd, reinterpret_cast<long>(working_directory),
                  sizeof(working_directory)) != 2 ||
      working_directory[0] != '/' || working_directory[1] != '\0') {
    return false;
  }
  return descriptor_inventory_is_exact() &&
         limit_matches(RLIMIT_AS, kAddressSpaceLimit) &&
         limit_matches(RLIMIT_DATA, kDataLimit) &&
         limit_matches(RLIMIT_STACK, kStackLimit) &&
         limit_matches(RLIMIT_CPU, 30U) && limit_matches(RLIMIT_FSIZE, 0U) &&
         limit_matches(RLIMIT_CORE, 0U) && limit_matches(RLIMIT_NOFILE, 8U) &&
         limit_matches(RLIMIT_NPROC, 1U);
}

[[noreturn]] void serve_ready_worker(const std::uintptr_t* stack) noexcept {
  if (!runtime_inventory_is_exact(stack) ||
      !write_all(kWorkerReadyDescriptor, kWorkerReadyAttestation.data(),
                 kWorkerReadyAttestation.size())) {
    exit_worker(90);
  }
  static_cast<void>(raw_syscall(__NR_close, kWorkerReadyDescriptor));
  std::byte buffer[64]{};
  while (true) {
    const long amount = raw_syscall(
        __NR_read, kWorkerChannelDescriptor, reinterpret_cast<long>(buffer),
        sizeof(buffer));
    if (amount == 0) {
      exit_worker(0);
    }
    if (amount == kInterrupted || amount == kTryAgain) {
      struct pollfd input{kWorkerChannelDescriptor, POLLIN, 0};
      static_cast<void>(raw_syscall(__NR_poll, reinterpret_cast<long>(&input),
                                    1, -1));
      continue;
    }
    if (amount < 0 ||
        !write_all(kWorkerChannelDescriptor, buffer,
                   static_cast<std::size_t>(amount))) {
      exit_worker(91);
    }
  }
}

[[noreturn]] void execute_mode(const std::uintptr_t* stack) noexcept {
  if constexpr (OHL_LINUX_TEST_WORKER_MODE == 0) {
    serve_ready_worker(stack);
  } else if constexpr (OHL_LINUX_TEST_WORKER_MODE == 1) {
    auto bad_attestation = kWorkerReadyAttestation;
    bad_attestation[0] = std::byte{'X'};
    static_cast<void>(write_all(kWorkerReadyDescriptor, bad_attestation.data(),
                                bad_attestation.size()));
  } else if constexpr (OHL_LINUX_TEST_WORKER_MODE == 2) {
    static_cast<void>(raw_syscall(__NR_close, kWorkerReadyDescriptor));
  } else if constexpr (OHL_LINUX_TEST_WORKER_MODE == 3) {
    struct stat status {};
    static_cast<void>(raw_syscall(__NR_newfstatat, AT_FDCWD,
                                  reinterpret_cast<long>("/"),
                                  reinterpret_cast<long>(&status), 0));
  } else if constexpr (OHL_LINUX_TEST_WORKER_MODE == 4) {
    static_cast<void>(raw_syscall(__NR_openat, AT_FDCWD,
                                  reinterpret_cast<long>("/"), O_RDONLY, 0));
  } else if constexpr (OHL_LINUX_TEST_WORKER_MODE == 5) {
    static_cast<void>(raw_syscall(__NR_socket, 2, 1, 0));
  } else if constexpr (OHL_LINUX_TEST_WORKER_MODE == 6) {
    static_cast<void>(raw_syscall(__NR_fork));
  } else if constexpr (OHL_LINUX_TEST_WORKER_MODE == 7) {
    static_cast<void>(raw_syscall(__NR_execveat, kWorkerChannelDescriptor,
                                  reinterpret_cast<long>(""), 0, 0,
                                  AT_EMPTY_PATH));
  } else if constexpr (OHL_LINUX_TEST_WORKER_MODE == 8) {
    struct pollfd channel{kWorkerChannelDescriptor, POLLIN, 0};
    static_cast<void>(raw_syscall(__NR_poll, reinterpret_cast<long>(&channel),
                                  1, -1));
    exit_worker(92);
  } else if constexpr (OHL_LINUX_TEST_WORKER_MODE == 9) {
    static_cast<void>(write_all(kWorkerReadyDescriptor,
                                kWorkerReadyAttestation.data(),
                                kWorkerReadyAttestation.size()));
    struct pollfd channel{kWorkerChannelDescriptor, POLLIN, 0};
    static_cast<void>(raw_syscall(__NR_poll, reinterpret_cast<long>(&channel),
                                  1, -1));
  } else if constexpr (OHL_LINUX_TEST_WORKER_MODE == 10) {
    static_cast<void>(write_all(kWorkerReadyDescriptor,
                                kWorkerReadyAttestation.data(),
                                kWorkerReadyAttestation.size()));
    constexpr std::byte trailing{0x5a};
    static_cast<void>(write_all(kWorkerReadyDescriptor, &trailing, 1U));
    static_cast<void>(raw_syscall(__NR_close, kWorkerReadyDescriptor));
    exit_worker(92);
  } else if constexpr (OHL_LINUX_TEST_WORKER_MODE == 11) {
    static_cast<void>(write_all(kWorkerReadyDescriptor,
                                kWorkerReadyAttestation.data(),
                                kWorkerReadyAttestation.size() / 2U));
    static_cast<void>(raw_syscall(__NR_close, kWorkerReadyDescriptor));
    exit_worker(92);
  } else if constexpr (OHL_LINUX_TEST_WORKER_MODE == 12) {
    constexpr auto high_dirfd =
        (static_cast<std::uint64_t>(1U) << 32U) |
        static_cast<std::uint64_t>(5U);
    static_cast<void>(raw_syscall(
        __NR_execveat, static_cast<long>(high_dirfd),
        reinterpret_cast<long>(""), 0, 0, AT_EMPTY_PATH));
  } else if constexpr (OHL_LINUX_TEST_WORKER_MODE == 13) {
    static_cast<void>(raw_syscall(__NR_execveat, 5,
                                  reinterpret_cast<long>(""), 0, 0, 0));
  } else if constexpr (OHL_LINUX_TEST_WORKER_MODE == 14) {
    constexpr auto high_flags =
        (static_cast<std::uint64_t>(1U) << 32U) |
        static_cast<std::uint64_t>(AT_EMPTY_PATH);
    static_cast<void>(raw_syscall(
        __NR_execveat, 5, reinterpret_cast<long>(""), 0, 0,
        static_cast<long>(high_flags)));
  }
  static_cast<void>(write_all(kWorkerReadyDescriptor,
                              kWorkerReadyAttestation.data(),
                              kWorkerReadyAttestation.size()));
  static_cast<void>(raw_syscall(__NR_close, kWorkerReadyDescriptor));
  exit_worker(92);
}

}  // namespace

extern "C" [[noreturn]] void ohl_linux_test_worker_main(
    const std::uintptr_t* initial_stack) noexcept {
  execute_mode(initial_stack);
}

__asm__(R"(
  .global _start
  .type _start,@function
_start:
  mov %rsp, %rdi
  and $-16, %rsp
  call ohl_linux_test_worker_main
  ud2
  .size _start, .-_start
)");
