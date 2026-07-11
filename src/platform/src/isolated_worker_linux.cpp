#include "isolated_worker_linux_internal.hpp"

#include "isolated_worker_internal.hpp"

#include <asm/unistd.h>
#include <elf.h>
#include <fcntl.h>
#include <linux/audit.h>
#include <linux/filter.h>
#include <linux/landlock.h>
#include <linux/seccomp.h>
#include <poll.h>
#include <signal.h>
#include <sys/prctl.h>
#include <sys/resource.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <unistd.h>

#include <algorithm>
#include <array>
#include <atomic>
#include <cerrno>
#include <chrono>
#include <climits>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <limits>
#include <memory>
#include <span>
#include <string_view>
#include <utility>

#if defined(OHL_LINUX_ISOLATED_WORKER_TESTING)
#ifndef OHL_LINUX_TEST_MEDIA_PARSER_WORKER_PATH
#error "Linux isolated-worker tests require one compile-fixed worker path"
#endif
#endif

namespace ohl::platform::detail {
namespace {

using Clock = std::chrono::steady_clock;

constexpr int kWorkerChannelFd =
    linux_isolated_worker::kWorkerChannelDescriptor;
constexpr int kBootstrapStatusFd =
    linux_isolated_worker::kWorkerReadyDescriptor;
constexpr int kWorkerImageFd = 5;
constexpr int kMinimumTemporaryFd = 10;
constexpr std::string_view kProductionWorkerPath =
    "/usr/libexec/open-half-life/ohl-media-parser-worker";

constexpr rlim_t kAddressSpaceLimit = 512U * 1024U * 1024U;
constexpr rlim_t kDataLimit = 256U * 1024U * 1024U;
constexpr rlim_t kStackLimit = 8U * 1024U * 1024U;
constexpr rlim_t kCpuSecondsLimit = 30U;
constexpr rlim_t kOpenFileLimit = 8U;
constexpr rlim_t kProcessLimit = 1U;

enum class BootstrapFailure : std::uint8_t {
  descriptor_setup = 1,
  working_directory = 2,
  resource_limits = 3,
  no_new_privileges = 4,
  landlock = 5,
  seccomp = 6,
  execute = 7,
};

class UniqueFd final {
 public:
  UniqueFd() noexcept = default;
  explicit UniqueFd(const int value) noexcept : value_(value) {}
  ~UniqueFd() {
    if (value_ >= 0) {
      static_cast<void>(::close(value_));
    }
  }

  UniqueFd(const UniqueFd&) = delete;
  UniqueFd& operator=(const UniqueFd&) = delete;
  UniqueFd(UniqueFd&& other) noexcept
      : value_(std::exchange(other.value_, -1)) {}
  UniqueFd& operator=(UniqueFd&& other) noexcept {
    if (this != &other) {
      reset(std::exchange(other.value_, -1));
    }
    return *this;
  }

  [[nodiscard]] int get() const noexcept { return value_; }
  [[nodiscard]] bool valid() const noexcept { return value_ >= 0; }
  [[nodiscard]] int release() noexcept { return std::exchange(value_, -1); }
  void reset(const int value = -1) noexcept {
    if (value_ >= 0) {
      static_cast<void>(::close(value_));
    }
    value_ = value;
  }

 private:
  int value_{-1};
};

[[nodiscard]] int duplicate_temporary_fd(const int descriptor) noexcept {
  return ::fcntl(descriptor, F_DUPFD_CLOEXEC, kMinimumTemporaryFd);
}

[[nodiscard]] bool set_limit(const int resource, const rlim_t value) noexcept {
  const struct rlimit limit {
    value, value
  };
  return ::setrlimit(resource, &limit) == 0;
}

[[noreturn]] void report_bootstrap_failure(
    const BootstrapFailure failure) noexcept {
  const auto value = static_cast<std::uint8_t>(failure);
  const auto ignored = ::write(kBootstrapStatusFd, &value, sizeof(value));
  static_cast<void>(ignored);
  ::_exit(127);
}

[[nodiscard]] bool install_resource_limits() noexcept {
  return set_limit(RLIMIT_AS, kAddressSpaceLimit) &&
         set_limit(RLIMIT_DATA, kDataLimit) &&
         set_limit(RLIMIT_STACK, kStackLimit) &&
         set_limit(RLIMIT_CPU, kCpuSecondsLimit) &&
         set_limit(RLIMIT_FSIZE, 0U) && set_limit(RLIMIT_CORE, 0U) &&
         set_limit(RLIMIT_NOFILE, kOpenFileLimit) &&
         set_limit(RLIMIT_NPROC, kProcessLimit);
}

[[nodiscard]] std::uint64_t landlock_access_rights(const int abi) noexcept {
  std::uint64_t rights = LANDLOCK_ACCESS_FS_EXECUTE |
                         LANDLOCK_ACCESS_FS_WRITE_FILE |
                         LANDLOCK_ACCESS_FS_READ_FILE |
                         LANDLOCK_ACCESS_FS_READ_DIR |
                         LANDLOCK_ACCESS_FS_REMOVE_DIR |
                         LANDLOCK_ACCESS_FS_REMOVE_FILE |
                         LANDLOCK_ACCESS_FS_MAKE_CHAR |
                         LANDLOCK_ACCESS_FS_MAKE_DIR |
                         LANDLOCK_ACCESS_FS_MAKE_REG |
                         LANDLOCK_ACCESS_FS_MAKE_SOCK |
                         LANDLOCK_ACCESS_FS_MAKE_FIFO |
                         LANDLOCK_ACCESS_FS_MAKE_BLOCK |
                         LANDLOCK_ACCESS_FS_MAKE_SYM;
  if (abi >= 2) {
    rights |= LANDLOCK_ACCESS_FS_REFER;
  }
#if defined(LANDLOCK_ACCESS_FS_TRUNCATE)
  if (abi >= 3) {
    rights |= LANDLOCK_ACCESS_FS_TRUNCATE;
  }
#endif
#if defined(LANDLOCK_ACCESS_FS_IOCTL_DEV)
  if (abi >= 5) {
    rights |= LANDLOCK_ACCESS_FS_IOCTL_DEV;
  }
#endif
  return rights;
}

[[nodiscard]] bool install_landlock(const int worker_image_fd) noexcept {
  const int abi = static_cast<int>(
      ::syscall(__NR_landlock_create_ruleset, nullptr, 0U,
                LANDLOCK_CREATE_RULESET_VERSION));
  if (abi < 1) {
    return false;
  }

  struct landlock_ruleset_attr ruleset_attributes {};
  ruleset_attributes.handled_access_fs = landlock_access_rights(abi);
  UniqueFd ruleset{static_cast<int>(::syscall(
      __NR_landlock_create_ruleset, &ruleset_attributes,
      sizeof(ruleset_attributes), 0U))};
  if (!ruleset.valid()) {
    return false;
  }

  struct landlock_path_beneath_attr worker_rule {};
  worker_rule.allowed_access =
      LANDLOCK_ACCESS_FS_EXECUTE | LANDLOCK_ACCESS_FS_READ_FILE;
  worker_rule.parent_fd = worker_image_fd;
  if (::syscall(__NR_landlock_add_rule, ruleset.get(),
                LANDLOCK_RULE_PATH_BENEATH, &worker_rule, 0U) != 0) {
    return false;
  }
  return ::syscall(__NR_landlock_restrict_self, ruleset.get(), 0U) == 0;
}

#define OHL_SECCOMP_ALLOW(syscall_name)                                      \
  BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_##syscall_name, 0, 1),           \
      BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW)

[[nodiscard]] bool install_seccomp() noexcept {
  struct sock_filter filter[] = {
      BPF_STMT(BPF_LD | BPF_W | BPF_ABS,
               static_cast<unsigned int>(offsetof(struct seccomp_data, arch))),
      BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, AUDIT_ARCH_X86_64, 1, 0),
      BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS),
      BPF_STMT(BPF_LD | BPF_W | BPF_ABS,
               static_cast<unsigned int>(offsetof(struct seccomp_data, nr))),
      BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_execveat, 0, 13),
      BPF_STMT(BPF_LD | BPF_W | BPF_ABS,
               static_cast<unsigned int>(
                   offsetof(struct seccomp_data, args[0]))),
      BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, kWorkerImageFd, 1, 0),
      BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS),
      BPF_STMT(BPF_LD | BPF_W | BPF_ABS,
               static_cast<unsigned int>(
                   offsetof(struct seccomp_data, args[0]) + sizeof(std::uint32_t))),
      BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, 0, 1, 0),
      BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS),
      BPF_STMT(BPF_LD | BPF_W | BPF_ABS,
               static_cast<unsigned int>(
                   offsetof(struct seccomp_data, args[4]))),
      BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, AT_EMPTY_PATH, 1, 0),
      BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS),
      BPF_STMT(BPF_LD | BPF_W | BPF_ABS,
               static_cast<unsigned int>(
                   offsetof(struct seccomp_data, args[4]) + sizeof(std::uint32_t))),
      BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, 0, 1, 0),
      BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS),
      BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
      OHL_SECCOMP_ALLOW(read),
      OHL_SECCOMP_ALLOW(write),
      OHL_SECCOMP_ALLOW(close),
      OHL_SECCOMP_ALLOW(exit),
      OHL_SECCOMP_ALLOW(exit_group),
      OHL_SECCOMP_ALLOW(poll),
      OHL_SECCOMP_ALLOW(ppoll),
      OHL_SECCOMP_ALLOW(recvfrom),
      OHL_SECCOMP_ALLOW(sendto),
      OHL_SECCOMP_ALLOW(clock_gettime),
      OHL_SECCOMP_ALLOW(clock_nanosleep),
      OHL_SECCOMP_ALLOW(futex),
      OHL_SECCOMP_ALLOW(set_tid_address),
      OHL_SECCOMP_ALLOW(set_robust_list),
      OHL_SECCOMP_ALLOW(rseq),
      OHL_SECCOMP_ALLOW(arch_prctl),
      OHL_SECCOMP_ALLOW(brk),
      OHL_SECCOMP_ALLOW(mmap),
      OHL_SECCOMP_ALLOW(mprotect),
      OHL_SECCOMP_ALLOW(munmap),
      OHL_SECCOMP_ALLOW(madvise),
      OHL_SECCOMP_ALLOW(rt_sigaction),
      OHL_SECCOMP_ALLOW(rt_sigprocmask),
      OHL_SECCOMP_ALLOW(rt_sigreturn),
      OHL_SECCOMP_ALLOW(sigaltstack),
      OHL_SECCOMP_ALLOW(getrandom),
      OHL_SECCOMP_ALLOW(getpid),
      OHL_SECCOMP_ALLOW(gettid),
      OHL_SECCOMP_ALLOW(getcwd),
      OHL_SECCOMP_ALLOW(sched_yield),
      OHL_SECCOMP_ALLOW(fstat),
      BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_prlimit64, 0, 13),
      BPF_STMT(BPF_LD | BPF_W | BPF_ABS,
               static_cast<unsigned int>(
                   offsetof(struct seccomp_data, args[0]))),
      BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, 0, 1, 0),
      BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS),
      BPF_STMT(BPF_LD | BPF_W | BPF_ABS,
               static_cast<unsigned int>(
                   offsetof(struct seccomp_data, args[0]) + sizeof(std::uint32_t))),
      BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, 0, 1, 0),
      BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS),
      BPF_STMT(BPF_LD | BPF_W | BPF_ABS,
               static_cast<unsigned int>(
                   offsetof(struct seccomp_data, args[2]))),
      BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, 0, 1, 0),
      BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS),
      BPF_STMT(BPF_LD | BPF_W | BPF_ABS,
               static_cast<unsigned int>(
                   offsetof(struct seccomp_data, args[2]) + sizeof(std::uint32_t))),
      BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, 0, 1, 0),
      BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS),
      BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
      BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS),
  };
  const struct sock_fprog program {
    static_cast<unsigned short>(sizeof(filter) / sizeof(filter[0])), filter
  };
  return ::prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &program) == 0;
}

#undef OHL_SECCOMP_ALLOW

[[noreturn]] void run_child(const int channel_fd, const int status_fd,
                            const int worker_image_fd,
                            const int root_fd) noexcept {
  if (::fchdir(root_fd) != 0) {
    report_bootstrap_failure(BootstrapFailure::working_directory);
  }
  if (::dup3(channel_fd, kWorkerChannelFd, 0) < 0 ||
      ::dup3(status_fd, kBootstrapStatusFd, 0) < 0 ||
      ::dup3(worker_image_fd, kWorkerImageFd, O_CLOEXEC) < 0) {
    report_bootstrap_failure(BootstrapFailure::descriptor_setup);
  }
  if (::syscall(__NR_close_range, 0U, 2U, 0U) != 0 ||
      ::syscall(__NR_close_range, 6U, UINT_MAX, 0U) != 0) {
    report_bootstrap_failure(BootstrapFailure::descriptor_setup);
  }
  if (!install_resource_limits()) {
    report_bootstrap_failure(BootstrapFailure::resource_limits);
  }
  if (::prctl(PR_SET_NO_NEW_PRIVS, 1L, 0L, 0L, 0L) != 0) {
    report_bootstrap_failure(BootstrapFailure::no_new_privileges);
  }
  if (!install_landlock(kWorkerImageFd)) {
    report_bootstrap_failure(BootstrapFailure::landlock);
  }
  if (!install_seccomp()) {
    report_bootstrap_failure(BootstrapFailure::seccomp);
  }

  char executable_name[] = "ohl-media-parser-worker";
  char* const arguments[] = {executable_name, nullptr};
  char* const environment[] = {nullptr};
  static_cast<void>(::syscall(__NR_execveat, kWorkerImageFd, "", arguments,
                              environment, AT_EMPTY_PATH));
  report_bootstrap_failure(BootstrapFailure::execute);
}

[[nodiscard]] bool verify_static_worker_elf(const int descriptor,
                                            const struct stat& status) noexcept {
  if (status.st_size < static_cast<off_t>(sizeof(Elf64_Ehdr))) {
    return false;
  }

  Elf64_Ehdr header{};
  if (::pread(descriptor, &header, sizeof(header), 0) !=
          static_cast<ssize_t>(sizeof(header)) ||
      std::memcmp(header.e_ident, ELFMAG, SELFMAG) != 0 ||
      header.e_ident[EI_CLASS] != ELFCLASS64 ||
      header.e_ident[EI_DATA] != ELFDATA2LSB || header.e_machine != EM_X86_64 ||
      (header.e_type != ET_EXEC && header.e_type != ET_DYN) ||
      header.e_phentsize != sizeof(Elf64_Phdr) || header.e_phnum == 0U ||
      header.e_phnum > 1024U) {
    return false;
  }

  constexpr auto program_header_size =
      static_cast<std::uint64_t>(sizeof(Elf64_Phdr));
  if (static_cast<std::uint64_t>(header.e_phnum) >
      std::numeric_limits<std::uint64_t>::max() / program_header_size) {
    return false;
  }
  const auto table_size =
      static_cast<std::uint64_t>(header.e_phnum) * program_header_size;
  const auto file_size = static_cast<std::uint64_t>(status.st_size);
  const auto table_offset = static_cast<std::uint64_t>(header.e_phoff);
  if (table_offset > file_size || table_size > file_size - table_offset) {
    return false;
  }
  for (Elf64_Half index = 0; index < header.e_phnum; ++index) {
    Elf64_Phdr program_header{};
    const auto offset = static_cast<off_t>(
        header.e_phoff + static_cast<Elf64_Off>(index) * sizeof(Elf64_Phdr));
    if (::pread(descriptor, &program_header, sizeof(program_header), offset) !=
        static_cast<ssize_t>(sizeof(program_header))) {
      return false;
    }
    if (program_header.p_type == PT_INTERP) {
      return false;
    }
  }
  return true;
}

[[nodiscard]] bool safe_worker_metadata(const struct stat& status,
                                        const bool production) noexcept {
  const bool trusted_owner =
      production ? status.st_uid == 0U
                 : (status.st_uid == 0U || status.st_uid == ::geteuid());
  return S_ISREG(status.st_mode) && trusted_owner &&
         (status.st_mode & (S_IWUSR | S_IWGRP | S_IWOTH)) == 0 &&
         (status.st_mode & (S_ISUID | S_ISGID)) == 0 &&
         (status.st_mode & (S_IXUSR | S_IXGRP | S_IXOTH)) != 0;
}

#if !defined(OHL_LINUX_ISOLATED_WORKER_TESTING)
[[nodiscard]] bool safe_production_directory(const int descriptor) noexcept {
  struct stat status {};
  return ::fstat(descriptor, &status) == 0 && S_ISDIR(status.st_mode) &&
         status.st_uid == 0U &&
         (status.st_mode & (S_IWGRP | S_IWOTH | S_ISUID | S_ISGID)) == 0;
}

[[nodiscard]] UniqueFd open_production_worker(
    IsolatedWorkerError& error) noexcept {
  UniqueFd directory{
      ::open("/", O_RDONLY | O_DIRECTORY | O_CLOEXEC | O_NOFOLLOW)};
  if (!directory.valid() || !safe_production_directory(directory.get())) {
    error = IsolatedWorkerError::service_identity_mismatch;
    return {};
  }
  constexpr const char* components[]{"usr", "libexec", "open-half-life"};
  for (const char* component : components) {
    UniqueFd next{::openat(directory.get(), component,
                           O_RDONLY | O_DIRECTORY | O_CLOEXEC | O_NOFOLLOW)};
    if (!next.valid()) {
      error = errno == ENOENT ? IsolatedWorkerError::service_unavailable
                              : IsolatedWorkerError::service_identity_mismatch;
      return {};
    }
    if (!safe_production_directory(next.get())) {
      error = IsolatedWorkerError::service_identity_mismatch;
      return {};
    }
    directory = std::move(next);
  }
  UniqueFd worker{::openat(directory.get(), "ohl-media-parser-worker",
                           O_RDONLY | O_CLOEXEC | O_NOFOLLOW)};
  if (!worker.valid()) {
    error = errno == ENOENT ? IsolatedWorkerError::service_unavailable
                            : IsolatedWorkerError::service_identity_mismatch;
  }
  return worker;
}
#endif

[[nodiscard]] UniqueFd open_trusted_worker(
    IsolatedWorkerError& error) noexcept {
#if defined(OHL_LINUX_ISOLATED_WORKER_TESTING)
  UniqueFd worker{
      ::open(OHL_LINUX_TEST_MEDIA_PARSER_WORKER_PATH,
             O_RDONLY | O_CLOEXEC | O_NOFOLLOW)};
  if (!worker.valid()) {
    error = errno == ENOENT ? IsolatedWorkerError::service_unavailable
                            : IsolatedWorkerError::service_identity_mismatch;
  }
  return worker;
#else
  return open_production_worker(error);
#endif
}

[[nodiscard]] int deadline_poll_timeout_ms(
    const Clock::time_point deadline) noexcept {
  const auto now = Clock::now();
  if (deadline <= now) {
    return 0;
  }
  const auto remaining = deadline - now;
  const auto milliseconds =
      std::chrono::duration_cast<std::chrono::milliseconds>(remaining);
  auto timeout = milliseconds.count();
  if (milliseconds < remaining) {
    ++timeout;
  }
  return static_cast<int>(std::min<std::int64_t>(timeout, INT_MAX));
}

[[nodiscard]] bool poll_until_ready(const int descriptor, const short events,
                                    const Clock::time_point deadline,
                                    short& returned_events) noexcept {
  while (true) {
    struct pollfd item {
      descriptor, events, 0
    };
    const int result = ::poll(&item, 1U, deadline_poll_timeout_ms(deadline));
    if (result > 0) {
      returned_events = item.revents;
      return true;
    }
    if (result == 0) {
      return false;
    }
    if (errno != EINTR) {
      returned_events = POLLNVAL;
      return true;
    }
    if (deadline <= Clock::now()) {
      return false;
    }
  }
}

[[nodiscard]] bool poll_io_until_ready(
    const int descriptor, const short events, const Clock::time_point deadline,
    const IsolatedWorkerCancellationToken& cancellation,
    short& returned_events) noexcept {
  using namespace std::chrono_literals;
  while (!cancellation.cancellation_requested()) {
    const auto now = Clock::now();
    if (deadline <= now) {
      return false;
    }
    const auto slice_deadline = std::min(deadline, now + 10ms);
    if (poll_until_ready(descriptor, events, slice_deadline,
                         returned_events)) {
      return true;
    }
  }
  return false;
}

[[nodiscard]] int pidfd_open(const pid_t process) noexcept {
  return static_cast<int>(::syscall(__NR_pidfd_open, process, 0U));
}

[[nodiscard]] int pidfd_send_kill(const int pidfd) noexcept {
  return static_cast<int>(
      ::syscall(__NR_pidfd_send_signal, pidfd, SIGKILL, nullptr, 0U));
}

void kill_and_reap(const pid_t process, const int pidfd) noexcept {
  if (pidfd >= 0) {
    if (pidfd_send_kill(pidfd) != 0 && errno != ESRCH) {
      static_cast<void>(::kill(process, SIGKILL));
    }
  } else {
    static_cast<void>(::kill(process, SIGKILL));
  }
  int status = 0;
  while (::waitpid(process, &status, 0) < 0 && errno == EINTR) {
  }
}

class LinuxIsolatedWorkerBackend final : public IsolatedWorkerBackend {
 public:
  LinuxIsolatedWorkerBackend(const pid_t process, UniqueFd pidfd,
                             UniqueFd channel) noexcept
      : process_(process),
        pidfd_(std::move(pidfd)),
        channel_(std::move(channel)) {}

  ~LinuxIsolatedWorkerBackend() override {
    request_termination();
    if (!reaped_) {
      if (termination_signal_failed_.load()) {
        static_cast<void>(::kill(process_, SIGKILL));
      }
      int status = 0;
      while (::waitpid(process_, &status, 0) < 0 && errno == EINTR) {
      }
    }
  }

  [[nodiscard]] IsolatedWorkerIoResult read_exact(
      const std::span<std::byte> destination,
      const Clock::time_point deadline,
      const IsolatedWorkerCancellationToken cancellation) noexcept override {
    std::size_t transferred = 0;
    while (transferred < destination.size()) {
      short events = 0;
      if (!poll_io_until_ready(channel_.get(), POLLIN, deadline, cancellation,
                               events)) {
        return {.bytes_transferred = transferred,
                .error = cancellation.cancellation_requested()
                             ? IsolatedWorkerError::cancelled
                             : IsolatedWorkerError::timeout};
      }
      if ((events & POLLNVAL) != 0) {
        return {.bytes_transferred = transferred,
                .error = IsolatedWorkerError::io_failure};
      }
      const auto amount = ::recv(channel_.get(), destination.data() + transferred,
                                 destination.size() - transferred, 0);
      if (amount > 0) {
        transferred += static_cast<std::size_t>(amount);
        continue;
      }
      if (amount == 0) {
        return {.bytes_transferred = transferred,
                .error = aborted_.load() ? IsolatedWorkerError::cancelled
                                         : IsolatedWorkerError::peer_closed};
      }
      if (errno == EINTR || errno == EAGAIN || errno == EWOULDBLOCK) {
        continue;
      }
      return {.bytes_transferred = transferred,
              .error = aborted_.load() ? IsolatedWorkerError::cancelled
                                       : IsolatedWorkerError::io_failure};
    }
    return {.bytes_transferred = transferred};
  }

  [[nodiscard]] IsolatedWorkerIoResult write_all(
      const std::span<const std::byte> source,
      const Clock::time_point deadline,
      const IsolatedWorkerCancellationToken cancellation) noexcept override {
    std::size_t transferred = 0;
    while (transferred < source.size()) {
      short events = 0;
      if (!poll_io_until_ready(channel_.get(), POLLOUT, deadline, cancellation,
                               events)) {
        return {.bytes_transferred = transferred,
                .error = cancellation.cancellation_requested()
                             ? IsolatedWorkerError::cancelled
                             : IsolatedWorkerError::timeout};
      }
      if ((events & POLLNVAL) != 0) {
        return {.bytes_transferred = transferred,
                .error = IsolatedWorkerError::io_failure};
      }
      const auto amount =
          ::send(channel_.get(), source.data() + transferred,
                 source.size() - transferred, MSG_NOSIGNAL);
      if (amount > 0) {
        transferred += static_cast<std::size_t>(amount);
        continue;
      }
      if (amount < 0 &&
          (errno == EINTR || errno == EAGAIN || errno == EWOULDBLOCK)) {
        continue;
      }
      return {.bytes_transferred = transferred,
              .error = aborted_.load() ? IsolatedWorkerError::cancelled
                                       : IsolatedWorkerError::peer_closed};
    }
    return {.bytes_transferred = transferred};
  }

  void abort_io() noexcept override {
    aborted_.store(true);
    close_channel();
  }

  void close_channel() noexcept override {
    if (!channel_shutdown_.exchange(true)) {
      static_cast<void>(::shutdown(channel_.get(), SHUT_RDWR));
    }
  }

  void request_termination() noexcept override {
    aborted_.store(true);
    close_channel();
    if (termination_requested_.exchange(true)) {
      return;
    }
    while (pidfd_send_kill(pidfd_.get()) != 0) {
      if (errno == EINTR) {
        continue;
      }
      if (errno != ESRCH) {
        termination_signal_failed_.store(true);
      }
      return;
    }
    termination_signal_sent_.store(true);
  }

  [[nodiscard]] IsolatedWorkerWaitResult wait(
      const Clock::time_point deadline) noexcept override {
    return wait_for_exit(deadline);
  }

  [[nodiscard]] IsolatedWorkerWaitResult terminate_and_wait(
      const Clock::time_point deadline) noexcept override {
    request_termination();
    auto result = wait_for_exit(deadline);
    if (result.error == IsolatedWorkerError::timeout &&
        termination_signal_failed_.load()) {
      result = {.exit = IsolatedWorkerExitKind::unknown,
                .error = IsolatedWorkerError::termination_failed};
    }
    return result;
  }

 private:
  [[nodiscard]] IsolatedWorkerWaitResult wait_for_exit(
      const Clock::time_point deadline) noexcept {
    if (reaped_) {
      return {.exit = cached_exit_};
    }
    short events = 0;
    if (!poll_until_ready(pidfd_.get(), POLLIN, deadline, events)) {
      return {.exit = IsolatedWorkerExitKind::running,
              .error = IsolatedWorkerError::timeout};
    }
    if ((events & POLLNVAL) != 0) {
      request_termination();
      return {.exit = IsolatedWorkerExitKind::unknown,
              .error = IsolatedWorkerError::reap_failed};
    }

    int wait_status = 0;
    pid_t waited = -1;
    do {
      waited = ::waitpid(process_, &wait_status, WNOHANG);
    } while (waited < 0 && errno == EINTR);
    if (waited != process_) {
      request_termination();
      return {.exit = IsolatedWorkerExitKind::unknown,
              .error = IsolatedWorkerError::reap_failed};
    }
    cached_exit_ = linux_isolated_worker::classify_wait_status(
        wait_status,
        termination_signal_sent_.load());
    reaped_ = true;
    return {.exit = cached_exit_};
  }

  pid_t process_;
  UniqueFd pidfd_;
  UniqueFd channel_;
  std::atomic_bool aborted_{false};
  std::atomic_bool channel_shutdown_{false};
  std::atomic_bool termination_requested_{false};
  std::atomic_bool termination_signal_sent_{false};
  std::atomic_bool termination_signal_failed_{false};
  bool reaped_{false};
  IsolatedWorkerExitKind cached_exit_{IsolatedWorkerExitKind::unknown};
};

[[nodiscard]] IsolatedWorkerError bootstrap_failure_error(
    const BootstrapFailure failure) noexcept {
  switch (failure) {
    case BootstrapFailure::resource_limits:
    case BootstrapFailure::no_new_privileges:
    case BootstrapFailure::landlock:
    case BootstrapFailure::seccomp:
      return IsolatedWorkerError::confinement_unavailable;
    case BootstrapFailure::descriptor_setup:
    case BootstrapFailure::working_directory:
    case BootstrapFailure::execute:
      return IsolatedWorkerError::bootstrap_failed;
  }
  return IsolatedWorkerError::bootstrap_failed;
}

struct ReadyResult {
  bool ready{false};
  IsolatedWorkerError error{IsolatedWorkerError::bootstrap_failed};
};

[[nodiscard]] ReadyResult await_worker_ready(
    const int status_fd, const int pidfd,
    const Clock::time_point deadline) noexcept {
  std::array<std::byte, linux_isolated_worker::kWorkerReadyAttestation.size()>
      received{};
  std::size_t received_size = 0;
  while (deadline > Clock::now()) {
    struct pollfd descriptors[2]{{status_fd, POLLIN, 0},
                                 {pidfd, POLLIN, 0}};
    const int poll_result =
        ::poll(descriptors, 2U, deadline_poll_timeout_ms(deadline));
    if (poll_result < 0) {
      if (errno == EINTR) {
        continue;
      }
      return {.ready = false, .error = IsolatedWorkerError::bootstrap_failed};
    }
    if (poll_result == 0) {
      return {.ready = false, .error = IsolatedWorkerError::timeout};
    }
    if ((descriptors[0].revents & POLLNVAL) != 0 ||
        (descriptors[1].revents & POLLNVAL) != 0) {
      return {.ready = false, .error = IsolatedWorkerError::bootstrap_failed};
    }

    if ((descriptors[0].revents & (POLLIN | POLLHUP | POLLERR)) != 0) {
      while (true) {
        std::byte byte{};
        const auto amount = ::read(status_fd, &byte, sizeof(byte));
        if (amount == static_cast<ssize_t>(sizeof(byte))) {
          if (received_size >= received.size() ||
              byte != linux_isolated_worker::
                          kWorkerReadyAttestation[received_size]) {
            if (received_size == 0U) {
              return {.ready = false,
                      .error = bootstrap_failure_error(
                          static_cast<BootstrapFailure>(
                              std::to_integer<std::uint8_t>(byte)))};
            }
            return {.ready = false,
                    .error = IsolatedWorkerError::bootstrap_failed};
          }
          received[received_size] = byte;
          ++received_size;
          continue;
        }
        if (amount == 0) {
          return received_size == received.size()
                     ? ReadyResult{.ready = true,
                                   .error = IsolatedWorkerError::none}
                     : ReadyResult{
                           .ready = false,
                           .error = IsolatedWorkerError::bootstrap_failed};
        }
        if (errno == EINTR) {
          continue;
        }
        if (errno != EAGAIN && errno != EWOULDBLOCK) {
          return {.ready = false,
                  .error = IsolatedWorkerError::bootstrap_failed};
        }
        break;
      }
    }
    if ((descriptors[1].revents & (POLLIN | POLLHUP | POLLERR)) != 0 &&
        received_size != received.size()) {
      return {.ready = false, .error = IsolatedWorkerError::bootstrap_failed};
    }
  }
  return {.ready = false, .error = IsolatedWorkerError::timeout};
}

}  // namespace

namespace linux_isolated_worker {

std::string_view service_executable_path(
    const IsolatedWorkerService service) noexcept {
  switch (service) {
    case IsolatedWorkerService::media_parser:
#if defined(OHL_LINUX_ISOLATED_WORKER_TESTING)
      return OHL_LINUX_TEST_MEDIA_PARSER_WORKER_PATH;
#else
      return kProductionWorkerPath;
#endif
  }
  return {};
}

IsolatedWorkerExitKind classify_wait_status(
    const int wait_status,
    const bool termination_requested) noexcept {
  if (WIFEXITED(wait_status)) {
    return WEXITSTATUS(wait_status) == 0 ? IsolatedWorkerExitKind::clean
                                         : IsolatedWorkerExitKind::failed;
  }
  if (WIFSIGNALED(wait_status)) {
    const int signal = WTERMSIG(wait_status);
    if (termination_requested && signal == SIGKILL) {
      return IsolatedWorkerExitKind::terminated;
    }
    if (signal == SIGXCPU || signal == SIGXFSZ) {
      return IsolatedWorkerExitKind::resource_limit;
    }
    return IsolatedWorkerExitKind::crashed;
  }
  return IsolatedWorkerExitKind::unknown;
}

}  // namespace linux_isolated_worker

IsolatedWorkerBackendLaunchResult launch_isolated_worker_backend(
    const IsolatedWorkerService service,
    const Clock::time_point startup_deadline) noexcept {
  const auto path = linux_isolated_worker::service_executable_path(service);
  if (path.empty()) {
    return {.backend = nullptr,
            .error = IsolatedWorkerError::invalid_argument};
  }

  IsolatedWorkerError identity_error = IsolatedWorkerError::none;
  UniqueFd worker_image = open_trusted_worker(identity_error);
  if (!worker_image.valid()) {
    return {.backend = nullptr, .error = identity_error};
  }
  struct stat worker_status {};
  if (::fstat(worker_image.get(), &worker_status) != 0 ||
#if defined(OHL_LINUX_ISOLATED_WORKER_TESTING)
      !safe_worker_metadata(worker_status, false) ||
#else
      !safe_worker_metadata(worker_status, true) ||
#endif
      !verify_static_worker_elf(worker_image.get(), worker_status)) {
    return {.backend = nullptr,
            .error = IsolatedWorkerError::service_identity_mismatch};
  }

  UniqueFd root{::open("/", O_RDONLY | O_DIRECTORY | O_CLOEXEC | O_NOFOLLOW)};
  if (!root.valid()) {
    return {.backend = nullptr,
            .error = IsolatedWorkerError::confinement_unavailable};
  }

  int sockets[2]{-1, -1};
  if (::socketpair(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC | SOCK_NONBLOCK, 0,
                   sockets) != 0) {
    return {.backend = nullptr,
            .error = IsolatedWorkerError::channel_creation_failed};
  }
  UniqueFd parent_channel{sockets[0]};
  UniqueFd child_channel{sockets[1]};

  int status_pipe[2]{-1, -1};
  if (::pipe2(status_pipe, O_CLOEXEC | O_NONBLOCK) != 0) {
    return {.backend = nullptr,
            .error = IsolatedWorkerError::channel_creation_failed};
  }
  UniqueFd status_read{status_pipe[0]};
  UniqueFd status_write{status_pipe[1]};

  UniqueFd child_channel_copy{duplicate_temporary_fd(child_channel.get())};
  UniqueFd status_write_copy{duplicate_temporary_fd(status_write.get())};
  UniqueFd worker_image_copy{duplicate_temporary_fd(worker_image.get())};
  UniqueFd root_copy{duplicate_temporary_fd(root.get())};
  if (!child_channel_copy.valid() || !status_write_copy.valid() ||
      !worker_image_copy.valid() || !root_copy.valid()) {
    return {.backend = nullptr,
            .error = IsolatedWorkerError::resource_exhausted};
  }

  const pid_t process = ::fork();
  if (process < 0) {
    return {.backend = nullptr,
            .error = IsolatedWorkerError::process_creation_failed};
  }
  if (process == 0) {
    run_child(child_channel_copy.get(), status_write_copy.get(),
              worker_image_copy.get(), root_copy.get());
  }

  child_channel.reset();
  status_write.reset();
  child_channel_copy.reset();
  status_write_copy.reset();
  worker_image_copy.reset();
  root_copy.reset();
  worker_image.reset();
  root.reset();

  UniqueFd process_fd{pidfd_open(process)};
  if (!process_fd.valid()) {
    kill_and_reap(process, -1);
    return {.backend = nullptr,
            .error = IsolatedWorkerError::confinement_unavailable};
  }

  const auto ready = await_worker_ready(status_read.get(), process_fd.get(),
                                        startup_deadline);
  if (!ready.ready) {
    kill_and_reap(process, process_fd.get());
    return {.backend = nullptr, .error = ready.error};
  }

  try {
    auto backend = std::make_unique<LinuxIsolatedWorkerBackend>(
        process, std::move(process_fd), std::move(parent_channel));
    return {.backend = std::move(backend),
            .error = IsolatedWorkerError::none};
  } catch (...) {
    kill_and_reap(process, process_fd.get());
    return {.backend = nullptr,
            .error = IsolatedWorkerError::resource_exhausted};
  }
}

}  // namespace ohl::platform::detail
