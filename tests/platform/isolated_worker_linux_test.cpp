#include "isolated_worker_linux_internal.hpp"

#include "ohl/platform/isolated_worker.hpp"

#include <elf.h>
#include <signal.h>

#include <array>
#include <atomic>
#include <chrono>
#include <cstddef>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <limits>
#include <span>
#include <string_view>
#include <system_error>
#include <thread>
#include <vector>

#ifndef OHL_LINUX_TEST_WORKER_STAGE_PATH
#error "The Linux native test requires a compile-fixed staging path"
#endif

namespace {

using Clock = std::chrono::steady_clock;
using namespace std::chrono_literals;
using ohl::platform::IsolatedWorker;
using ohl::platform::IsolatedWorkerError;
using ohl::platform::IsolatedWorkerExitKind;
using ohl::platform::IsolatedWorkerService;

class TestContext final {
 public:
  void expect(const bool condition, const std::string_view message) {
    if (!condition) {
      std::cerr << "FAIL: " << message << '\n';
      ++failures_;
    }
  }

  [[nodiscard]] int result() const noexcept { return failures_ == 0 ? 0 : 1; }

 private:
  int failures_{0};
};

[[nodiscard]] bool stage_worker(const std::filesystem::path& source,
                                const bool owner_writable = false) {
  const std::filesystem::path destination{OHL_LINUX_TEST_WORKER_STAGE_PATH};
  const auto temporary = destination.string() + ".new";
  std::error_code error;
  std::filesystem::create_directories(destination.parent_path(), error);
  if (error) {
    return false;
  }
  std::filesystem::remove(temporary, error);
  error.clear();
  std::filesystem::copy_file(source, temporary,
                             std::filesystem::copy_options::overwrite_existing,
                             error);
  if (error) {
    return false;
  }
  auto permissions = std::filesystem::perms::owner_read |
                     std::filesystem::perms::owner_exec |
                     std::filesystem::perms::group_read |
                     std::filesystem::perms::group_exec |
                     std::filesystem::perms::others_read |
                     std::filesystem::perms::others_exec;
  if (owner_writable) {
    permissions |= std::filesystem::perms::owner_write;
  }
  std::filesystem::permissions(temporary, permissions,
                               std::filesystem::perm_options::replace, error);
  if (error) {
    return false;
  }
  std::filesystem::remove(destination, error);
  error.clear();
  std::filesystem::rename(temporary, destination, error);
  return !error;
}

[[nodiscard]] bool stage_worker_bytes(const std::span<const std::byte> bytes) {
  const std::filesystem::path destination{OHL_LINUX_TEST_WORKER_STAGE_PATH};
  const auto temporary = destination.string() + ".new";
  std::error_code error;
  std::filesystem::create_directories(destination.parent_path(), error);
  if (error) {
    return false;
  }
  {
    std::ofstream output{temporary, std::ios::binary | std::ios::trunc};
    if (!output) {
      return false;
    }
    output.write(reinterpret_cast<const char*>(bytes.data()),
                 static_cast<std::streamsize>(bytes.size()));
    if (!output) {
      return false;
    }
  }
  constexpr auto permissions = std::filesystem::perms::owner_read |
                               std::filesystem::perms::owner_exec |
                               std::filesystem::perms::group_read |
                               std::filesystem::perms::group_exec |
                               std::filesystem::perms::others_read |
                               std::filesystem::perms::others_exec;
  std::filesystem::permissions(temporary, permissions,
                               std::filesystem::perm_options::replace, error);
  if (error) {
    return false;
  }
  std::filesystem::remove(destination, error);
  error.clear();
  std::filesystem::rename(temporary, destination, error);
  return !error;
}

[[nodiscard]] std::vector<std::byte> load_worker_fixture(
    const std::filesystem::path& source) {
  std::ifstream input{source, std::ios::binary | std::ios::ate};
  if (!input) {
    return {};
  }
  const auto end = input.tellg();
  if (end <= 0) {
    return {};
  }
  std::vector<std::byte> bytes(static_cast<std::size_t>(end));
  input.seekg(0);
  input.read(reinterpret_cast<char*>(bytes.data()),
             static_cast<std::streamsize>(bytes.size()));
  return input ? bytes : std::vector<std::byte>{};
}

[[nodiscard]] Elf64_Ehdr load_elf_header(
    const std::span<const std::byte> bytes) {
  Elf64_Ehdr header{};
  if (bytes.size() >= sizeof(header)) {
    std::memcpy(&header, bytes.data(), sizeof(header));
  }
  return header;
}

void store_elf_header(std::span<std::byte> bytes,
                      const Elf64_Ehdr& header) {
  if (bytes.size() >= sizeof(header)) {
    std::memcpy(bytes.data(), &header, sizeof(header));
  }
}

[[nodiscard]] ohl::platform::IsolatedWorkerLaunchResult launch_worker(
    const std::chrono::milliseconds timeout = 2s) {
  return ohl::platform::launch_isolated_worker(IsolatedWorkerService::media_parser,
                                                Clock::now() + timeout);
}

void expect_bootstrap_rejection(TestContext& test, const char* helper,
                                const std::string_view message,
                                const std::chrono::milliseconds timeout = 2s,
                                const IsolatedWorkerError expected =
                                    IsolatedWorkerError::bootstrap_failed) {
  test.expect(stage_worker(helper), "stage adversarial worker");
  const auto launch = launch_worker(timeout);
  test.expect(!launch.valid() && launch.error == expected, message);
}

[[nodiscard]] std::unique_ptr<IsolatedWorker> require_ready_worker(
    TestContext& test) {
  test.expect(stage_worker(OHL_LINUX_TEST_WORKER_READY_PATH),
              "stage ready worker");
  auto launch = launch_worker();
  test.expect(launch.valid(),
              "exact runtime inventory and ready attestation launch");
  return std::move(launch.worker);
}

void test_identity_and_attestation(TestContext& test) {
  namespace native = ohl::platform::detail::linux_isolated_worker;
  const auto service_path =
      native::service_executable_path(IsolatedWorkerService::media_parser);
  test.expect(service_path == OHL_LINUX_TEST_WORKER_STAGE_PATH,
              "test backend retains one compile-fixed service identity");
  test.expect(native::service_executable_path(
                  static_cast<IsolatedWorkerService>(0xffU))
                  .empty(),
              "unknown services have no executable identity");

  test.expect(stage_worker(OHL_LINUX_TEST_WORKER_READY_PATH, true),
              "stage owner-writable identity probe");
  const auto writable = launch_worker();
  test.expect(!writable.valid() &&
                  writable.error ==
                      IsolatedWorkerError::service_identity_mismatch,
              "owner-writable worker identity is rejected");

  for (const auto permission : {std::filesystem::perms::set_uid,
                                std::filesystem::perms::set_gid}) {
    test.expect(stage_worker(OHL_LINUX_TEST_WORKER_READY_PATH),
                "stage set-id identity probe");
    std::error_code permission_error;
    std::filesystem::permissions(
        OHL_LINUX_TEST_WORKER_STAGE_PATH, permission,
        std::filesystem::perm_options::add, permission_error);
    test.expect(!permission_error, "apply set-id identity probe mode");
    const auto set_id = launch_worker();
    test.expect(!set_id.valid() &&
                    set_id.error ==
                        IsolatedWorkerError::service_identity_mismatch,
                "set-id worker identity is rejected");
  }

  expect_bootstrap_rejection(test, OHL_LINUX_TEST_WORKER_BAD_READY_PATH,
                             "malformed ready record fails launch");
  expect_bootstrap_rejection(test, OHL_LINUX_TEST_WORKER_NO_READY_PATH,
                             "EOF before ready record fails launch");
  expect_bootstrap_rejection(test, OHL_LINUX_TEST_WORKER_READY_TIMEOUT_PATH,
                             "missing ready record reaches startup timeout",
                             50ms, IsolatedWorkerError::timeout);
  expect_bootstrap_rejection(
      test, OHL_LINUX_TEST_WORKER_READY_HELD_OPEN_PATH,
      "valid ready record without framing EOF reaches startup timeout", 50ms,
      IsolatedWorkerError::timeout);
  expect_bootstrap_rejection(test, OHL_LINUX_TEST_WORKER_READY_TRAILING_PATH,
                             "trailing ready data fails exact framing");
  expect_bootstrap_rejection(test, OHL_LINUX_TEST_WORKER_READY_TRUNCATED_PATH,
                             "truncated ready prefix followed by EOF fails");
}

void test_elf_identity_rejections(TestContext& test) {
  const auto valid = load_worker_fixture(OHL_LINUX_TEST_WORKER_READY_PATH);
  test.expect(valid.size() >= sizeof(Elf64_Ehdr),
              "load authored static ELF identity fixture");
  if (valid.size() < sizeof(Elf64_Ehdr)) {
    return;
  }

  const auto expect_rejected = [&](const std::vector<std::byte>& fixture,
                                   const std::string_view message) {
    test.expect(stage_worker_bytes(fixture), "stage malformed ELF fixture");
    const auto launch = launch_worker();
    test.expect(!launch.valid() &&
                    launch.error ==
                        IsolatedWorkerError::service_identity_mismatch,
                message);
  };

  std::vector<std::byte> truncated(valid.begin(),
                                   valid.begin() + sizeof(Elf64_Ehdr) - 1U);
  expect_rejected(truncated, "truncated ELF header is rejected");

  auto table_out_of_file = valid;
  auto header = load_elf_header(table_out_of_file);
  header.e_phoff = static_cast<Elf64_Off>(table_out_of_file.size() - 1U);
  header.e_phnum = 1U;
  store_elf_header(table_out_of_file, header);
  expect_rejected(table_out_of_file,
                  "program-header table outside the file is rejected");

  auto table_overflow = valid;
  header = load_elf_header(table_overflow);
  header.e_phoff = std::numeric_limits<Elf64_Off>::max() - 8U;
  header.e_phnum = 2U;
  store_elf_header(table_overflow, header);
  expect_rejected(table_overflow,
                  "overflowing program-header table bounds are rejected");

  auto excessive_headers = valid;
  header = load_elf_header(excessive_headers);
  header.e_phnum = 1025U;
  store_elf_header(excessive_headers, header);
  expect_rejected(excessive_headers,
                  "excessive program-header count is rejected");

  auto wrong_machine = valid;
  header = load_elf_header(wrong_machine);
  header.e_machine = EM_AARCH64;
  store_elf_header(wrong_machine, header);
  expect_rejected(wrong_machine, "wrong ELF machine is rejected");

  auto wrong_class = valid;
  wrong_class[EI_CLASS] = std::byte{ELFCLASS32};
  expect_rejected(wrong_class, "wrong ELF class is rejected");

  auto interpreted = valid;
  header = load_elf_header(interpreted);
  const auto first_program_header = static_cast<std::size_t>(header.e_phoff);
  test.expect(first_program_header <= interpreted.size() &&
                  sizeof(Elf64_Phdr) <=
                      interpreted.size() - first_program_header,
              "authored fixture contains a program header");
  if (first_program_header <= interpreted.size() &&
      sizeof(Elf64_Phdr) <= interpreted.size() - first_program_header) {
    Elf64_Phdr program_header{};
    std::memcpy(&program_header,
                interpreted.data() +
                    static_cast<std::ptrdiff_t>(first_program_header),
                sizeof(program_header));
    program_header.p_type = PT_INTERP;
    std::memcpy(interpreted.data() +
                    static_cast<std::ptrdiff_t>(first_program_header),
                &program_header, sizeof(program_header));
    expect_rejected(interpreted, "PT_INTERP worker is rejected");
  }
}

void test_sanitized_status(TestContext& test) {
  namespace native = ohl::platform::detail::linux_isolated_worker;
  test.expect(native::classify_wait_status(0, false) ==
                  IsolatedWorkerExitKind::clean,
              "zero exit is clean");
  test.expect(native::classify_wait_status(7 << 8, false) ==
                  IsolatedWorkerExitKind::failed,
              "nonzero exit is sanitized as failed");
  test.expect(native::classify_wait_status(SIGSEGV, false) ==
                  IsolatedWorkerExitKind::crashed,
              "unexpected signal is sanitized as crashed");
  test.expect(native::classify_wait_status(SIGXCPU, false) ==
                  IsolatedWorkerExitKind::resource_limit,
              "limit signal is sanitized as resource-limit");
  test.expect(native::classify_wait_status(SIGKILL, true) ==
                  IsolatedWorkerExitKind::terminated,
              "owned kill is sanitized as terminated");
}

void test_denied_syscalls(TestContext& test) {
  struct Probe {
    const char* path;
    const char* message;
  };
  constexpr Probe probes[]{
      {OHL_LINUX_TEST_WORKER_FORBIDDEN_STAT_PATH,
       "path stat is killed before ready"},
      {OHL_LINUX_TEST_WORKER_FORBIDDEN_OPEN_PATH,
       "filesystem open is killed before ready"},
      {OHL_LINUX_TEST_WORKER_FORBIDDEN_NETWORK_PATH,
       "network socket creation is killed before ready"},
      {OHL_LINUX_TEST_WORKER_FORBIDDEN_FORK_PATH,
       "process creation is killed before ready"},
      {OHL_LINUX_TEST_WORKER_FORBIDDEN_REEXEC_PATH,
       "execveat outside fixed fd5/AT_EMPTY_PATH is killed"},
      {OHL_LINUX_TEST_WORKER_EXECVEAT_HIGH_DIRFD_PATH,
       "execveat with a nonzero dirfd high word is killed"},
      {OHL_LINUX_TEST_WORKER_EXECVEAT_WRONG_FLAGS_PATH,
       "execveat with wrong low flags is killed"},
      {OHL_LINUX_TEST_WORKER_EXECVEAT_HIGH_FLAGS_PATH,
       "execveat with a nonzero flags high word is killed"},
  };
  for (const auto& probe : probes) {
    expect_bootstrap_rejection(test, probe.path, probe.message);
  }
}

void test_io_abort_timeout_and_reap(TestContext& test) {
  {
    auto worker = require_ready_worker(test);
    if (worker != nullptr) {
      constexpr std::array payload{std::byte{0x12}, std::byte{0x34},
                                   std::byte{0x56}};
      std::array<std::byte, payload.size()> reply{};
      const auto written = worker->write_all(payload, Clock::now() + 1s);
      const auto read = worker->read_exact(reply, Clock::now() + 1s);
      test.expect(written.error == IsolatedWorkerError::none &&
                      written.bytes_transferred == payload.size() &&
                      read.error == IsolatedWorkerError::none &&
                      reply == payload,
                  "confined socketpair transfers exact duplex bytes");
      if (written.error != IsolatedWorkerError::none ||
          read.error != IsolatedWorkerError::none) {
        std::cerr << "native echo errors: write="
                  << static_cast<int>(written.error)
                  << " read=" << static_cast<int>(read.error) << '\n';
      }
      const auto terminated =
          worker->terminate_and_wait(Clock::now() + 2s);
      test.expect(terminated.error == IsolatedWorkerError::none &&
                      terminated.exit != IsolatedWorkerExitKind::running &&
                      terminated.exit != IsolatedWorkerExitKind::unknown,
                  "pidfd kill plus owned waitpid reap is terminal");
    }
  }

  {
    auto worker = require_ready_worker(test);
    if (worker != nullptr) {
      std::byte reply{};
      const auto timed_out = worker->read_exact(
          std::span<std::byte>{&reply, 1U}, Clock::now() + 30ms);
      test.expect(timed_out.error == IsolatedWorkerError::timeout,
                  "inactive worker read respects its deadline");
      if (timed_out.error != IsolatedWorkerError::timeout) {
        std::cerr << "native timeout error: "
                  << static_cast<int>(timed_out.error) << '\n';
      }
    }
  }

  {
    auto worker = require_ready_worker(test);
    if (worker != nullptr) {
      std::byte reply{};
      ohl::platform::IsolatedWorkerIoResult result{};
      std::thread reader{[&] {
        result = worker->read_exact(std::span<std::byte>{&reply, 1U},
                                    Clock::now() + 2s);
      }};
      std::this_thread::sleep_for(20ms);
      worker->abort_io();
      reader.join();
      test.expect(result.error == IsolatedWorkerError::cancelled,
                  "abort wakes and cancels active native I/O");
      if (result.error != IsolatedWorkerError::cancelled) {
        std::cerr << "native abort error: " << static_cast<int>(result.error)
                  << '\n';
      }
    }
  }

  {
    auto worker = require_ready_worker(test);
    if (worker != nullptr) {
      ohl::platform::IsolatedWorkerCancellationSource cancellation;
      const auto token = cancellation.token();
      std::atomic_bool reader_entered{false};
      std::byte reply{};
      ohl::platform::IsolatedWorkerIoResult result{};
      std::thread reader{[&] {
        reader_entered.store(true);
        result = worker->read_exact(std::span<std::byte>{&reply, 1U},
                                    Clock::now() + 2s, token);
      }};
      while (!reader_entered.load()) {
        std::this_thread::yield();
      }
      std::this_thread::sleep_for(20ms);
      std::byte concurrent_reply{};
      const auto active_probe = worker->read_exact(
          std::span<std::byte>{&concurrent_reply, 1U}, Clock::now() + 50ms);
      test.expect(active_probe.error ==
                      IsolatedWorkerError::concurrent_operation,
                  "token cancellation probe observes an active native read");
      const auto requested_at = Clock::now();
      const bool first_request = cancellation.request_cancellation();
      reader.join();
      const auto cancellation_latency = Clock::now() - requested_at;
      test.expect(first_request && result.error == IsolatedWorkerError::cancelled &&
                      result.bytes_transferred == 0U &&
                      cancellation_latency < 500ms,
                  "token-only cancellation promptly stops active native read "
                  "with zero partial bytes");

      constexpr std::array retry_payload{std::byte{0x7f}};
      const auto poisoned_read = worker->read_exact(
          std::span<std::byte>{&reply, 1U}, Clock::now() + 100ms);
      const auto poisoned_write =
          worker->write_all(retry_payload, Clock::now() + 100ms);
      test.expect(poisoned_read.error == IsolatedWorkerError::invalid_state &&
                      poisoned_write.error ==
                          IsolatedWorkerError::invalid_state,
                  "token cancellation permanently poisons the channel");
    }
  }
}

}  // namespace

int main() {
  TestContext test;
  test_identity_and_attestation(test);
  test_elf_identity_rejections(test);
  test_sanitized_status(test);
  test_denied_syscalls(test);
  test_io_abort_timeout_and_reap(test);
  return test.result();
}
