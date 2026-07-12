#include "isolated_worker_linux_internal.hpp"

#include "ohl/parser/protocol.hpp"
#include "ohl/parser/protocol_messages.hpp"

#include <fcntl.h>
#include <signal.h>
#include <sys/socket.h>
#include <sys/wait.h>
#include <unistd.h>

#include <algorithm>
#include <array>
#include <cerrno>
#include <chrono>
#include <cstddef>
#include <cstdint>
#include <iostream>
#include <span>
#include <string_view>
#include <thread>
#include <utility>
#include <vector>

#ifndef OHL_MEDIA_PARSER_WORKER_SERVICE_TEST_PATH
#error "The native service test requires a compile-fixed worker path"
#endif

namespace {

using namespace std::chrono_literals;
using ohl::platform::detail::linux_isolated_worker::kWorkerChannelDescriptor;
using ohl::platform::detail::linux_isolated_worker::kWorkerReadyAttestation;
using ohl::platform::detail::linux_isolated_worker::kWorkerReadyDescriptor;
namespace parser = ohl::parser;

constexpr std::uint64_t kSessionId = 0x4f484c5352563031ULL;
constexpr int kProtocolErrorExit = 64;
constexpr int kUnsupportedExit = 65;
constexpr int kTransportErrorExit = 66;

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
  ~UniqueFd() { reset(); }

  UniqueFd(const UniqueFd&) = delete;
  UniqueFd& operator=(const UniqueFd&) = delete;
  UniqueFd(UniqueFd&& other) noexcept : value_(other.release()) {}
  UniqueFd& operator=(UniqueFd&& other) noexcept {
    if (this != &other) {
      reset(other.release());
    }
    return *this;
  }

  [[nodiscard]] int get() const noexcept { return value_; }
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

struct ChildProcess {
  pid_t pid{-1};
  UniqueFd channel;
  UniqueFd readiness;

  ~ChildProcess() {
    if (pid <= 0) {
      return;
    }
    int status = 0;
    pid_t result = -1;
    do {
      result = ::waitpid(pid, &status, WNOHANG);
    } while (result < 0 && errno == EINTR);
    if (result == 0) {
      static_cast<void>(::kill(pid, SIGKILL));
      do {
        result = ::waitpid(pid, &status, 0);
      } while (result < 0 && errno == EINTR);
    }
  }
};

[[nodiscard]] ChildProcess spawn_worker(TestContext& test) {
  std::array<int, 2> channel{-1, -1};
  std::array<int, 2> readiness{-1, -1};
  if (::socketpair(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0, channel.data()) !=
          0 ||
      ::pipe2(readiness.data(), O_CLOEXEC) != 0) {
    test.expect(false, "create worker test descriptors");
    for (const int descriptor : channel) {
      if (descriptor >= 0) {
        static_cast<void>(::close(descriptor));
      }
    }
    for (const int descriptor : readiness) {
      if (descriptor >= 0) {
        static_cast<void>(::close(descriptor));
      }
    }
    return {};
  }

  const pid_t child = ::fork();
  if (child < 0) {
    test.expect(false, "fork worker service child");
    for (const int descriptor : channel) {
      static_cast<void>(::close(descriptor));
    }
    for (const int descriptor : readiness) {
      static_cast<void>(::close(descriptor));
    }
    return {};
  }
  if (child == 0) {
    const int channel_source = ::fcntl(channel[1], F_DUPFD_CLOEXEC, 10);
    const int readiness_source =
        ::fcntl(readiness[1], F_DUPFD_CLOEXEC, 10);
    for (const int descriptor : channel) {
      static_cast<void>(::close(descriptor));
    }
    for (const int descriptor : readiness) {
      static_cast<void>(::close(descriptor));
    }
    if (channel_source < 0 || readiness_source < 0 ||
        ::dup2(channel_source, kWorkerChannelDescriptor) < 0 ||
        ::dup2(readiness_source, kWorkerReadyDescriptor) < 0) {
      _exit(126);
    }
    static_cast<void>(::close(channel_source));
    static_cast<void>(::close(readiness_source));
    char executable_name[] = "ohl-media-parser-worker";
    char* const arguments[] = {executable_name, nullptr};
    char* const environment[] = {nullptr};
    ::execve(OHL_MEDIA_PARSER_WORKER_SERVICE_TEST_PATH, arguments, environment);
    _exit(127);
  }

  static_cast<void>(::close(channel[1]));
  static_cast<void>(::close(readiness[1]));
  return {.pid = child,
          .channel = UniqueFd{channel[0]},
          .readiness = UniqueFd{readiness[0]}};
}

[[nodiscard]] bool read_exact(const int descriptor,
                              const std::span<std::byte> destination) {
  std::size_t offset = 0;
  while (offset < destination.size()) {
    const ssize_t amount = ::read(
        descriptor,
        destination.data() + static_cast<std::ptrdiff_t>(offset),
        destination.size() - offset);
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
                             const std::span<const std::byte> source,
                             const bool fragmented = false) {
  std::size_t offset = 0;
  while (offset < source.size()) {
    const std::size_t requested = fragmented ? 1U : source.size() - offset;
    const ssize_t amount = ::write(
        descriptor, source.data() + static_cast<std::ptrdiff_t>(offset),
        requested);
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

[[nodiscard]] std::vector<std::byte> encode_frame(
    TestContext& test, const parser::FrameHeader& header,
    const std::span<const std::byte> payload = {}) {
  std::vector<std::byte> bytes(parser::kFrameHeaderBytes + payload.size());
  const auto result = parser::encode_frame(header, payload, bytes);
  test.expect(result.valid() && result.bytes_written == bytes.size(),
              "encode project-authored synthetic OWP/1 frame");
  return bytes;
}

[[nodiscard]] std::vector<std::byte> hello_frame(TestContext& test) {
  std::array<std::byte, parser::kHelloPayloadBytes> payload{};
  const auto encoded = parser::encode_hello_payload(
      {.source_size = 4096U, .maximum_read_bytes = 256U}, payload);
  test.expect(encoded.valid(), "encode synthetic hello payload");
  return encode_frame(
      test,
      {.type = parser::MessageType::hello,
       .payload_length = static_cast<std::uint32_t>(payload.size()),
       .session_id = kSessionId,
       .request_id = 0U},
      payload);
}

[[nodiscard]] bool receive_readiness(TestContext& test, ChildProcess& child) {
  std::array<std::byte, kWorkerReadyAttestation.size()> bytes{};
  const bool complete = read_exact(child.readiness.get(), bytes);
  test.expect(complete && bytes == kWorkerReadyAttestation,
              "worker emits exact readiness attestation");
  std::byte trailing{};
  const bool closed = ::read(child.readiness.get(), &trailing, 1) == 0;
  test.expect(closed, "worker closes readiness descriptor after attestation");
  child.readiness.reset();
  return complete && bytes == kWorkerReadyAttestation && closed;
}

[[nodiscard]] bool complete_handshake(TestContext& test, ChildProcess& child,
                                      const bool fragmented) {
  if (!receive_readiness(test, child)) {
    return false;
  }
  const auto hello = hello_frame(test);
  if (!write_all(child.channel.get(), hello, fragmented)) {
    test.expect(false, "write exact synthetic hello frame");
    return false;
  }
  std::array<std::byte, parser::kFrameHeaderBytes> bytes{};
  if (!read_exact(child.channel.get(), bytes)) {
    test.expect(false, "read exact ready frame header");
    return false;
  }
  const auto ready = parser::decode_frame_header(bytes);
  const bool canonical =
      ready.valid() && ready.header.type == parser::MessageType::ready &&
      ready.header.payload_length == 0U &&
      ready.header.session_id == kSessionId && ready.header.request_id == 0U;
  test.expect(canonical, "worker emits canonical OWP/1 ready frame");
  return canonical;
}

[[nodiscard]] bool wait_for_exit(TestContext& test, const pid_t child,
                                 const int expected_exit,
                                 const std::string_view lifetime) {
  const auto deadline = std::chrono::steady_clock::now() + 2s;
  int status = 0;
  while (std::chrono::steady_clock::now() < deadline) {
    const pid_t result = ::waitpid(child, &status, WNOHANG);
    if (result == child) {
      const bool expected = WIFEXITED(status) &&
                            WEXITSTATUS(status) == expected_exit;
      test.expect(expected, lifetime);
      errno = 0;
      test.expect(::waitpid(child, nullptr, WNOHANG) == -1 && errno == ECHILD,
                  "completed worker leaves no zombie child");
      return expected;
    }
    if (result < 0 && errno != EINTR) {
      test.expect(false, "waitpid failed before worker completion");
      return false;
    }
    std::this_thread::sleep_for(1ms);
  }
  static_cast<void>(::kill(child, SIGKILL));
  static_cast<void>(::waitpid(child, &status, 0));
  test.expect(false, "worker lifetime is bounded and reapable");
  return false;
}

void test_hello_ready_shutdown(TestContext& test) {
  auto child = spawn_worker(test);
  if (child.pid < 0 || !complete_handshake(test, child, true)) {
    return;
  }
  const auto shutdown = encode_frame(
      test, {.type = parser::MessageType::shutdown,
             .payload_length = 0U,
             .session_id = kSessionId,
             .request_id = 0U});
  test.expect(write_all(child.channel.get(), shutdown),
              "write canonical shutdown frame");
  std::byte trailing{};
  test.expect(::read(child.channel.get(), &trailing, 1) == 0,
              "clean shutdown closes fd 3");
  child.channel.reset();
  static_cast<void>(wait_for_exit(test, child.pid, 0,
                                  "clean service lifetime exits zero"));
}

void test_peer_close(TestContext& test) {
  auto child = spawn_worker(test);
  if (child.pid < 0 || !receive_readiness(test, child)) {
    return;
  }
  child.channel.reset();
  static_cast<void>(wait_for_exit(test, child.pid, 0,
                                  "orderly peer close exits cleanly"));
}

void test_malformed_header(TestContext& test) {
  auto child = spawn_worker(test);
  if (child.pid < 0 || !receive_readiness(test, child)) {
    return;
  }
  std::array<std::byte, parser::kFrameHeaderBytes> invalid{};
  test.expect(write_all(child.channel.get(), invalid),
              "write malformed synthetic header");
  static_cast<void>(wait_for_exit(test, child.pid, kProtocolErrorExit,
                                  "malformed frame has sanitized protocol exit"));
}

void test_truncated_header(TestContext& test) {
  auto child = spawn_worker(test);
  if (child.pid < 0 || !receive_readiness(test, child)) {
    return;
  }
  const auto hello = hello_frame(test);
  test.expect(write_all(child.channel.get(),
                        std::span<const std::byte>{hello}.first(7U)),
              "write truncated synthetic header");
  child.channel.reset();
  static_cast<void>(wait_for_exit(test, child.pid, kTransportErrorExit,
                                  "truncated header has sanitized I/O exit"));
}

void test_truncated_payload(TestContext& test) {
  auto child = spawn_worker(test);
  if (child.pid < 0 || !receive_readiness(test, child)) {
    return;
  }
  const auto hello = hello_frame(test);
  const auto partial_size = parser::kFrameHeaderBytes + 3U;
  test.expect(write_all(child.channel.get(),
                        std::span<const std::byte>{hello}.first(partial_size)),
              "write truncated synthetic payload");
  child.channel.reset();
  static_cast<void>(wait_for_exit(test, child.pid, kTransportErrorExit,
                                  "truncated payload has sanitized I/O exit"));
}

void test_unsupported_operation(TestContext& test) {
  auto child = spawn_worker(test);
  if (child.pid < 0 || !complete_handshake(test, child, false)) {
    return;
  }
  const auto enumerate = encode_frame(
      test, {.type = parser::MessageType::enumerate,
             .payload_length = 0U,
             .session_id = kSessionId,
             .request_id = 1U});
  test.expect(write_all(child.channel.get(), enumerate),
              "write unsupported enumerate operation");
  std::byte trailing{};
  test.expect(::read(child.channel.get(), &trailing, 1) == 0,
              "unsupported dispatcher aborts and closes fd 3");
  static_cast<void>(wait_for_exit(
      test, child.pid, kUnsupportedExit,
      "compile-fixed dispatcher returns sanitized unsupported exit"));
}

}  // namespace

int main() {
  static_cast<void>(::signal(SIGPIPE, SIG_IGN));
  TestContext test;
  test_hello_ready_shutdown(test);
  test_peer_close(test);
  test_malformed_header(test);
  test_truncated_header(test);
  test_truncated_payload(test);
  test_unsupported_operation(test);
  return test.result();
}
