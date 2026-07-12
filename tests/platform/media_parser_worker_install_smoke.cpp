#include "isolated_worker_linux_internal.hpp"

#include "ohl/parser/protocol.hpp"
#include "ohl/parser/protocol_messages.hpp"

#include <elf.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

#include <array>
#include <cerrno>
#include <chrono>
#include <cstddef>
#include <cstring>
#include <fstream>
#include <iostream>
#include <span>
#include <string_view>
#include <thread>
#include <vector>

namespace {

using namespace std::chrono_literals;
using ohl::platform::detail::linux_isolated_worker::kWorkerChannelDescriptor;
using ohl::platform::detail::linux_isolated_worker::kWorkerReadyAttestation;
using ohl::platform::detail::linux_isolated_worker::kWorkerReadyDescriptor;
namespace parser = ohl::parser;

constexpr std::uint64_t kSessionId = 0x4f484c4253543031ULL;

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

  [[nodiscard]] int get() const noexcept { return value_; }
  [[nodiscard]] int release() noexcept {
    const int value = value_;
    value_ = -1;
    return value;
  }
  void reset(const int value = -1) noexcept {
    if (value_ >= 0) {
      static_cast<void>(::close(value_));
    }
    value_ = value;
  }

 private:
  int value_{-1};
};

[[nodiscard]] bool read_exact(const int descriptor, std::span<std::byte> data) {
  std::size_t offset = 0;
  while (offset < data.size()) {
    const ssize_t amount =
        ::read(descriptor, data.data() + static_cast<std::ptrdiff_t>(offset),
               data.size() - offset);
    if (amount > 0) {
      offset += static_cast<std::size_t>(amount);
      continue;
    }
    if (amount < 0 && errno == EINTR) {
      continue;
    }
    return false;
  }
  return true;
}

[[nodiscard]] bool write_all(const int descriptor,
                             const std::span<const std::byte> data) {
  std::size_t offset = 0;
  while (offset < data.size()) {
    const ssize_t amount = ::write(
        descriptor, data.data() + static_cast<std::ptrdiff_t>(offset),
        data.size() - offset);
    if (amount > 0) {
      offset += static_cast<std::size_t>(amount);
      continue;
    }
    if (amount < 0 && errno == EINTR) {
      continue;
    }
    return false;
  }
  return true;
}

[[nodiscard]] bool write_frame(const int descriptor,
                               const parser::FrameHeader& header,
                               const std::span<const std::byte> payload = {}) {
  std::vector<std::byte> frame(parser::kFrameHeaderBytes + payload.size());
  const auto encoded = parser::encode_frame(header, payload, frame);
  return encoded.valid() && encoded.bytes_written == frame.size() &&
         write_all(descriptor, frame);
}

[[nodiscard]] std::vector<std::byte> read_file(const char* path) {
  std::ifstream input{path, std::ios::binary | std::ios::ate};
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

template <typename T>
[[nodiscard]] T load_object(const std::span<const std::byte> bytes,
                            const std::size_t offset) {
  T value{};
  if (offset <= bytes.size() && sizeof(T) <= bytes.size() - offset) {
    std::memcpy(&value, bytes.data() + static_cast<std::ptrdiff_t>(offset),
                sizeof(T));
  }
  return value;
}

[[nodiscard]] bool is_static_x86_64_elf(const std::span<const std::byte> bytes) {
  if (bytes.size() < sizeof(Elf64_Ehdr)) {
    return false;
  }
  const auto header = load_object<Elf64_Ehdr>(bytes, 0);
  if (std::memcmp(header.e_ident, ELFMAG, SELFMAG) != 0 ||
      header.e_ident[EI_CLASS] != ELFCLASS64 ||
      header.e_ident[EI_DATA] != ELFDATA2LSB || header.e_machine != EM_X86_64 ||
      (header.e_type != ET_EXEC && header.e_type != ET_DYN) ||
      header.e_phentsize != sizeof(Elf64_Phdr) || header.e_phnum == 0U) {
    return false;
  }
  const std::size_t table_offset = static_cast<std::size_t>(header.e_phoff);
  const std::size_t table_size =
      static_cast<std::size_t>(header.e_phnum) * sizeof(Elf64_Phdr);
  if (table_offset > bytes.size() || table_size > bytes.size() - table_offset) {
    return false;
  }
  for (Elf64_Half index = 0; index < header.e_phnum; ++index) {
    const auto program_header = load_object<Elf64_Phdr>(
        bytes, table_offset + static_cast<std::size_t>(index) *
                                  sizeof(Elf64_Phdr));
    if (program_header.p_type == PT_INTERP) {
      return false;
    }
  }
  return true;
}

[[nodiscard]] bool has_static_protocol_payload_storage(
    const std::span<const std::byte> bytes) {
  if (bytes.size() < sizeof(Elf64_Ehdr)) {
    return false;
  }
  const auto header = load_object<Elf64_Ehdr>(bytes, 0);
  if (header.e_shentsize != sizeof(Elf64_Shdr) || header.e_shnum == 0U) {
    return false;
  }
  const std::size_t table_offset = static_cast<std::size_t>(header.e_shoff);
  const std::size_t table_size =
      static_cast<std::size_t>(header.e_shnum) * sizeof(Elf64_Shdr);
  if (table_offset > bytes.size() || table_size > bytes.size() - table_offset) {
    return false;
  }
  constexpr std::uint64_t required_storage =
      2ULL * parser::kMaximumFramePayloadBytes;
  for (Elf64_Half index = 0; index < header.e_shnum; ++index) {
    const auto section = load_object<Elf64_Shdr>(
        bytes, table_offset +
                   static_cast<std::size_t>(index) * sizeof(Elf64_Shdr));
    if (section.sh_type == SHT_NOBITS && (section.sh_flags & SHF_ALLOC) != 0U &&
        section.sh_size >= required_storage) {
      return true;
    }
  }
  return false;
}

[[nodiscard]] bool installed_mode_is_immutable_executable(const char* path) {
  struct stat status {};
  return ::stat(path, &status) == 0 && S_ISREG(status.st_mode) &&
         (status.st_mode & (S_IWUSR | S_IWGRP | S_IWOTH)) == 0 &&
         (status.st_mode & (S_IXUSR | S_IXGRP | S_IXOTH)) != 0 &&
         (status.st_mode & (S_ISUID | S_ISGID)) == 0;
}

[[nodiscard]] bool wait_clean(const pid_t child) {
  int status = 0;
  while (::waitpid(child, &status, 0) < 0) {
    if (errno != EINTR) {
      return false;
    }
  }
  return WIFEXITED(status) && WEXITSTATUS(status) == 0;
}

void test_installed_artifact(TestContext& test, const char* worker_path) {
  test.expect(installed_mode_is_immutable_executable(worker_path),
              "installed worker is executable and has no write or set-id bits");
  const auto bytes = read_file(worker_path);
  test.expect(is_static_x86_64_elf(bytes),
              "installed worker is static x86-64 ELF without PT_INTERP");
  test.expect(has_static_protocol_payload_storage(bytes),
              "installed worker carries both maximum payload arenas in BSS");
}

void test_direct_lifecycle(TestContext& test, const char* worker_path) {
  std::array<int, 2> channel{-1, -1};
  std::array<int, 2> ready{-1, -1};
  test.expect(::socketpair(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0,
                           channel.data()) == 0,
              "create lifecycle socketpair");
  test.expect(::pipe2(ready.data(), O_CLOEXEC) == 0, "create readiness pipe");
  if (channel[0] < 0 || channel[1] < 0 || ready[0] < 0 || ready[1] < 0) {
    return;
  }

  UniqueFd parent_channel{channel[0]};
  UniqueFd child_channel{channel[1]};
  UniqueFd ready_read{ready[0]};
  UniqueFd ready_write{ready[1]};

  const pid_t child = ::fork();
  test.expect(child >= 0, "fork installed worker smoke child");
  if (child < 0) {
    return;
  }
  if (child == 0) {
    static_cast<void>(::close(parent_channel.get()));
    static_cast<void>(::close(ready_read.get()));
    if (::dup2(child_channel.get(), kWorkerChannelDescriptor) < 0 ||
        ::dup2(ready_write.get(), kWorkerReadyDescriptor) < 0) {
      _exit(126);
    }
    static_cast<void>(::fcntl(kWorkerChannelDescriptor, F_SETFD, 0));
    static_cast<void>(::fcntl(kWorkerReadyDescriptor, F_SETFD, 0));
    if (child_channel.get() != kWorkerChannelDescriptor &&
        child_channel.get() != kWorkerReadyDescriptor) {
      static_cast<void>(::close(child_channel.get()));
    }
    if (ready_write.get() != kWorkerChannelDescriptor &&
        ready_write.get() != kWorkerReadyDescriptor) {
      static_cast<void>(::close(ready_write.get()));
    }
    char executable_name[] = "ohl-media-parser-worker";
    char* const arguments[] = {executable_name, nullptr};
    char* const environment[] = {nullptr};
    ::execve(worker_path, arguments, environment);
    _exit(127);
  }

  child_channel.reset();
  ready_write.reset();

  std::array<std::byte, kWorkerReadyAttestation.size()> attestation{};
  test.expect(read_exact(ready_read.get(), attestation),
              "installed worker emits complete readiness attestation");
  test.expect(attestation == kWorkerReadyAttestation,
              "installed worker readiness attestation is exact");

  std::byte trailing{};
  test.expect(::read(ready_read.get(), &trailing, 1) == 0,
              "installed worker closes readiness fd after attestation");

  std::array<std::byte, parser::kHelloPayloadBytes> hello_payload{};
  const auto hello = parser::encode_hello_payload(
      {.source_size = 4096U, .maximum_read_bytes = 256U}, hello_payload);
  test.expect(hello.valid(), "encode synthetic installed-worker hello");
  test.expect(
      write_frame(parent_channel.get(),
                  {.type = parser::MessageType::hello,
                   .payload_length =
                       static_cast<std::uint32_t>(hello_payload.size()),
                   .session_id = kSessionId,
                   .request_id = 0U},
                  hello_payload),
      "installed worker accepts a complete OWP/1 hello on fd 3");

  std::array<std::byte, parser::kFrameHeaderBytes> ready_bytes{};
  test.expect(read_exact(parent_channel.get(), ready_bytes),
              "installed worker emits a complete OWP/1 ready header");
  const auto ready_frame = parser::decode_frame_header(ready_bytes);
  test.expect(ready_frame.valid() &&
                  ready_frame.header.type == parser::MessageType::ready &&
                  ready_frame.header.payload_length == 0U &&
                  ready_frame.header.session_id == kSessionId &&
                  ready_frame.header.request_id == 0U,
              "installed worker ready frame is exact and canonical");

  test.expect(write_frame(parent_channel.get(),
                          {.type = parser::MessageType::shutdown,
                           .payload_length = 0U,
                           .session_id = kSessionId,
                           .request_id = 0U}),
              "installed worker accepts canonical shutdown");
  std::byte channel_trailing{};
  test.expect(::read(parent_channel.get(), &channel_trailing, 1) == 0,
              "installed worker closes fd 3 after canonical shutdown");
  parent_channel.reset();
  if (!wait_clean(child)) {
    static_cast<void>(::kill(child, SIGKILL));
    static_cast<void>(::waitpid(child, nullptr, 0));
    test.expect(false, "installed worker exits cleanly after fd 3 closes");
  }
}

}  // namespace

int main(const int argc, char** argv) {
  if (argc != 2) {
    std::cerr << "usage: " << argv[0] << " INSTALLED_WORKER\n";
    return 2;
  }
  TestContext test;
  test_installed_artifact(test, argv[1]);
  test_direct_lifecycle(test, argv[1]);
  return test.result();
}
