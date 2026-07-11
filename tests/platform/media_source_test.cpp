#include "ohl/platform/media_source.hpp"

#include <algorithm>
#include <array>
#include <atomic>
#include <cerrno>
#include <chrono>
#include <cstddef>
#include <cstdint>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <limits>
#include <memory>
#include <span>
#include <string>
#include <string_view>
#include <system_error>
#include <thread>
#include <utility>
#include <vector>

#if !defined(OHL_PLATFORM_MEDIA_SOURCE_UNSUPPORTED) && \
    (defined(__linux__) || defined(__APPLE__))
#include <fcntl.h>
#if defined(__APPLE__)
#include <util.h>
#else
#include <pty.h>
#endif
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>
#endif

#if !defined(OHL_PLATFORM_MEDIA_SOURCE_UNSUPPORTED) && defined(_WIN32)
#if !defined(NOMINMAX)
#define NOMINMAX
#endif
#include <windows.h>
#if !defined(SYMBOLIC_LINK_FLAG_ALLOW_UNPRIVILEGED_CREATE)
#define SYMBOLIC_LINK_FLAG_ALLOW_UNPRIVILEGED_CREATE 0x2
#endif
#endif

#if defined(OHL_PLATFORM_MEDIA_SOURCE_UNSUPPORTED)

int main() {
  const auto opened = ohl::platform::open_media_source(
      std::filesystem::path{"ohl-synthetic-unsupported-source.fixture"});
  if (opened.source == nullptr && !opened.valid() &&
      opened.error == ohl::platform::MediaSourceError::unsupported) {
    return 0;
  }
  std::cerr << "media source test failed: unsupported platform contract\n";
  return 1;
}

#else

namespace {

using ohl::platform::MediaSource;
using ohl::platform::MediaSourceError;

class TemporaryDirectory final {
 public:
  TemporaryDirectory() {
    std::error_code error;
    const auto parent = std::filesystem::temp_directory_path(error);
    if (error) {
      return;
    }
    const auto nonce = std::chrono::steady_clock::now()
                           .time_since_epoch()
                           .count();
    for (unsigned int attempt = 0; attempt < 128; ++attempt) {
      auto candidate =
          parent / ("ohl-media-source-test-" + std::to_string(nonce) + "-" +
                    std::to_string(attempt));
      error.clear();
      if (std::filesystem::create_directory(candidate, error)) {
        path_ = std::move(candidate);
        return;
      }
      if (error && error != std::errc::file_exists) {
        return;
      }
    }
  }

  ~TemporaryDirectory() {
    if (!path_.empty()) {
      std::error_code error;
      std::filesystem::remove_all(path_, error);
    }
  }

  TemporaryDirectory(const TemporaryDirectory&) = delete;
  TemporaryDirectory& operator=(const TemporaryDirectory&) = delete;

  [[nodiscard]] bool valid() const noexcept { return !path_.empty(); }
  [[nodiscard]] const std::filesystem::path& path() const noexcept {
    return path_;
  }

 private:
  std::filesystem::path path_;
};

[[nodiscard]] std::vector<std::byte> make_bytes(const std::size_t size,
                                                 const unsigned int seed) {
  std::vector<std::byte> result(size);
  for (std::size_t index = 0; index < size; ++index) {
    result[index] = static_cast<std::byte>(
        (static_cast<unsigned int>(index) * 37U + seed) & 0xffU);
  }
  return result;
}

[[nodiscard]] bool write_bytes(const std::filesystem::path& path,
                               const std::span<const std::byte> bytes) {
  std::ofstream output{path, std::ios::binary | std::ios::trunc};
  for (const auto value : bytes) {
    output.put(static_cast<char>(std::to_integer<unsigned char>(value)));
  }
  return output.good();
}

[[nodiscard]] bool fail(const std::string_view message) {
  std::cerr << "media source test failed: " << message << '\n';
  return false;
}

[[nodiscard]] bool expect_error(const MediaSourceError actual,
                                const MediaSourceError expected,
                                const std::string_view context) {
  return actual == expected ? true : fail(context);
}

[[nodiscard]] bool expect_bytes(
    const std::shared_ptr<const MediaSource>& source,
    const std::uint64_t offset, const std::span<const std::byte> expected,
    const std::string_view context) {
  std::vector<std::byte> actual(expected.size());
  if (source->read_exact_at(offset, actual) != MediaSourceError::none ||
      !std::equal(actual.begin(), actual.end(), expected.begin(),
                  expected.end())) {
    return fail(context);
  }
  return true;
}

[[nodiscard]] bool test_open_errors(const TemporaryDirectory& temporary) {
  const auto missing = ohl::platform::open_media_source(
      temporary.path() / "missing-source.fixture");
  if (missing.source != nullptr || missing.valid() ||
      missing.error != MediaSourceError::not_found) {
    return fail("missing path classification");
  }

  const auto directory = ohl::platform::open_media_source(temporary.path());
  if (directory.source != nullptr || directory.valid() ||
      directory.error != MediaSourceError::not_regular_file) {
    return fail("directory classification");
  }
  return true;
}

[[nodiscard]] bool test_final_link_rejection(
    const TemporaryDirectory& temporary) {
  const auto target = temporary.path() / "link-target.fixture";
  const auto link = temporary.path() / "selected-link.fixture";
  const auto bytes = make_bytes(97, 7U);
  if (!write_bytes(target, bytes)) {
    return fail("final-link target creation");
  }

#if defined(_WIN32)
  if (CreateSymbolicLinkW(link.c_str(), target.c_str(),
                          SYMBOLIC_LINK_FLAG_ALLOW_UNPRIVILEGED_CREATE) == 0) {
    const auto error = GetLastError();
    if (error != ERROR_INVALID_PARAMETER ||
        CreateSymbolicLinkW(link.c_str(), target.c_str(), 0) == 0) {
      return fail("final reparse-point creation");
    }
  }
#else
  std::error_code error;
  std::filesystem::create_symlink(target.filename(), link, error);
  if (error) {
    return fail("final symlink creation");
  }
#endif

  const auto opened = ohl::platform::open_media_source(link);
  if (opened.source != nullptr || opened.valid() ||
      opened.error == MediaSourceError::none) {
    return fail("final link was not rejected");
  }
  return true;
}

#if defined(__linux__) || defined(__APPLE__)
[[nodiscard]] bool test_posix_fifo_rejection(
    const TemporaryDirectory& temporary) {
  const auto path = temporary.path() / "synthetic-source.fifo";
  if (::mkfifo(path.c_str(), 0600) != 0) {
    return fail("synthetic FIFO creation");
  }
  const auto opened = ohl::platform::open_media_source(path);
  if (opened.source != nullptr || opened.valid() ||
      opened.error != MediaSourceError::not_regular_file) {
    return fail("synthetic FIFO classification");
  }
  return true;
}

[[nodiscard]] bool test_posix_terminal_rejection() {
  int master = -1;
  int terminal = -1;
  std::array<char, 1'024> terminal_path{};
  if (::openpty(&master, &terminal, terminal_path.data(), nullptr, nullptr) !=
      0) {
    return fail("synthetic PTY creation");
  }

  const auto child = ::fork();
  if (child < 0) {
    (void)::close(master);
    (void)::close(terminal);
    return fail("synthetic PTY child creation");
  }
  if (child == 0) {
    const bool session_created = ::setsid() >= 0;
    (void)::close(terminal);
    const auto opened = ohl::platform::open_media_source(
        std::filesystem::path{terminal_path.data()});
    const bool rejected =
        opened.source == nullptr && !opened.valid() &&
        opened.error == MediaSourceError::not_regular_file;
    const int controlling_terminal =
        ::open("/dev/tty", O_RDONLY | O_NONBLOCK | O_NOCTTY);
    if (controlling_terminal >= 0) {
      (void)::close(controlling_terminal);
    }
    (void)::close(master);
    ::_exit(session_created && rejected && controlling_terminal < 0 ? 0 : 1);
  }

  int status = 0;
  pid_t waited = -1;
  do {
    waited = ::waitpid(child, &status, 0);
  } while (waited < 0 && errno == EINTR);
  (void)::close(master);
  (void)::close(terminal);
  if (waited != child || !WIFEXITED(status) ||
      WEXITSTATUS(status) != 0) {
    return fail("synthetic terminal acquisition contract");
  }
  return true;
}
#endif

[[nodiscard]] bool test_empty_file(const TemporaryDirectory& temporary) {
  const auto path = temporary.path() / "empty.fixture";
  if (!write_bytes(path, {})) {
    return fail("empty fixture creation");
  }
  const auto opened = ohl::platform::open_media_source(path);
  if (!opened.valid() || opened.source->size() != 0U ||
      opened.source->verify_unchanged() != MediaSourceError::none) {
    return fail("empty source acquisition");
  }

  std::array<std::byte, 1> one_byte{};
  const std::span<std::byte> empty{};
  return expect_error(opened.source->read_exact_at(0, empty),
                      MediaSourceError::none, "empty read at empty EOF") &&
         expect_error(opened.source->read_exact_at(1, empty),
                      MediaSourceError::out_of_range,
                      "empty read beyond empty EOF") &&
         expect_error(opened.source->read_exact_at(0, one_byte),
                      MediaSourceError::out_of_range,
                      "non-empty read from empty source");
}

[[nodiscard]] bool test_exact_and_boundary_reads(
    const TemporaryDirectory& temporary) {
  const auto path = temporary.path() / "bounded.fixture";
  const auto bytes = make_bytes(257, 11U);
  if (!write_bytes(path, bytes)) {
    return fail("bounded fixture creation");
  }
  const auto opened = ohl::platform::open_media_source(path);
  if (!opened.valid() || opened.source->size() != bytes.size()) {
    return fail("bounded source acquisition");
  }

  std::array<std::byte, 1> one_byte{};
  std::array<std::byte, 2> two_bytes{};
  std::array<std::byte, 4> four_bytes{};
  const std::span<std::byte> empty{};
  return expect_bytes(opened.source, 0, bytes, "whole-file read") &&
         expect_bytes(opened.source, 31,
                      std::span<const std::byte>{bytes}.subspan(31, 73),
                      "middle read") &&
         expect_bytes(opened.source, bytes.size() - 1,
                      std::span<const std::byte>{bytes}.last(1),
                      "last-byte read") &&
         expect_error(opened.source->read_exact_at(bytes.size(), empty),
                      MediaSourceError::none, "empty read at EOF") &&
         expect_error(opened.source->read_exact_at(bytes.size() + 1, empty),
                      MediaSourceError::out_of_range,
                      "empty read beyond EOF") &&
         expect_error(opened.source->read_exact_at(bytes.size(), one_byte),
                      MediaSourceError::out_of_range, "read starting at EOF") &&
         expect_error(opened.source->read_exact_at(bytes.size() - 1, two_bytes),
                      MediaSourceError::out_of_range,
                      "range extending beyond EOF") &&
         expect_error(opened.source->read_exact_at(
                          std::numeric_limits<std::uint64_t>::max(), one_byte),
                      MediaSourceError::out_of_range,
                      "maximum offset read") &&
         expect_error(opened.source->read_exact_at(
                          std::numeric_limits<std::uint64_t>::max(), empty),
                      MediaSourceError::out_of_range,
                      "maximum offset empty read") &&
         expect_error(opened.source->read_exact_at(
                          std::numeric_limits<std::uint64_t>::max() - 1U,
                          four_bytes),
                      MediaSourceError::out_of_range,
                      "overflowing range read");
}

[[nodiscard]] bool test_path_replacement(const TemporaryDirectory& temporary) {
  const auto selected_path = temporary.path() / "selected.fixture";
  const auto moved_path = temporary.path() / "selected-original.fixture";
  const auto original_bytes = make_bytes(193, 23U);
  const auto replacement_bytes = make_bytes(original_bytes.size(), 97U);
  if (!write_bytes(selected_path, original_bytes)) {
    return fail("replacement fixture creation");
  }
  const auto opened = ohl::platform::open_media_source(selected_path);
  if (!opened.valid()) {
    return fail("replacement source acquisition");
  }

  std::error_code error;
  std::filesystem::rename(selected_path, moved_path, error);
  if (error || !write_bytes(selected_path, replacement_bytes)) {
    return fail("path replacement setup");
  }
  return expect_bytes(opened.source, 0, original_bytes,
                      "pinned bytes after path replacement") &&
         expect_error(opened.source->verify_unchanged(), MediaSourceError::none,
                      "unchanged pinned object after path replacement");
}

[[nodiscard]] bool test_same_object_truncation(
    const TemporaryDirectory& temporary) {
  const auto path = temporary.path() / "truncated.fixture";
  const auto bytes = make_bytes(128, 31U);
  if (!write_bytes(path, bytes)) {
    return fail("truncation fixture creation");
  }
  const auto opened = ohl::platform::open_media_source(path);
  if (!opened.valid()) {
    return fail("truncation source acquisition");
  }

  std::error_code error;
  std::filesystem::resize_file(path, 17, error);
  if (error) {
    return fail("same-object truncation setup");
  }
  std::vector<std::byte> captured_size_read(bytes.size());
  return expect_error(opened.source->verify_unchanged(),
                      MediaSourceError::changed,
                      "same-object truncation detection") &&
         expect_error(opened.source->read_exact_at(0, captured_size_read),
                      MediaSourceError::unexpected_eof,
                      "early EOF after same-object truncation");
}

[[nodiscard]] bool test_same_object_rewrite(
    const TemporaryDirectory& temporary) {
  const auto path = temporary.path() / "rewritten.fixture";
  const auto bytes = make_bytes(128, 41U);
  if (!write_bytes(path, bytes)) {
    return fail("rewrite fixture creation");
  }
  const auto opened = ohl::platform::open_media_source(path);
  if (!opened.valid()) {
    return fail("rewrite source acquisition");
  }
  std::error_code error;
  const auto original_write_time = std::filesystem::last_write_time(path, error);
  if (error) {
    return fail("rewrite timestamp query");
  }

  {
    std::fstream file{path, std::ios::binary | std::ios::in | std::ios::out};
    const char replacement = static_cast<char>(0xa5);
    file.seekp(19);
    file.write(&replacement, 1);
    if (!file.good()) {
      return fail("same-object rewrite setup");
    }
  }
  std::filesystem::last_write_time(path,
                                   original_write_time + std::chrono::seconds{2},
                                   error);
  if (error) {
    return fail("rewrite timestamp update");
  }
  return expect_error(opened.source->verify_unchanged(),
                      MediaSourceError::changed,
                      "same-object rewrite detection");
}

[[nodiscard]] bool test_shared_lifetime(const TemporaryDirectory& temporary) {
  const auto path = temporary.path() / "shared.fixture";
  const auto bytes = make_bytes(211, 53U);
  if (!write_bytes(path, bytes)) {
    return fail("shared fixture creation");
  }
  auto opened = ohl::platform::open_media_source(path);
  if (!opened.valid()) {
    return fail("shared source acquisition");
  }
  auto surviving_reference = opened.source;
  opened.source.reset();
  if (opened.valid()) {
    return fail("open result remained valid without its source");
  }
  return expect_bytes(surviving_reference, 0, bytes,
                      "shared reference after result release") &&
         expect_error(surviving_reference->verify_unchanged(),
                      MediaSourceError::none,
                      "shared reference verification");
}

[[nodiscard]] bool test_concurrent_positional_reads(
    const TemporaryDirectory& temporary) {
  constexpr std::size_t kThreadCount = 8;
  constexpr std::size_t kIterations = 512;
  constexpr std::size_t kReadSize = 79;
  const auto path = temporary.path() / "concurrent.fixture";
  const auto bytes = make_bytes(64U * 1024U, 67U);
  if (!write_bytes(path, bytes)) {
    return fail("concurrent fixture creation");
  }
  const auto opened = ohl::platform::open_media_source(path);
  if (!opened.valid()) {
    return fail("concurrent source acquisition");
  }

  std::atomic<bool> failed{false};
  std::vector<std::thread> workers;
  workers.reserve(kThreadCount);
  for (std::size_t thread_index = 0; thread_index < kThreadCount;
       ++thread_index) {
    workers.emplace_back([&, thread_index] {
      std::array<std::byte, kReadSize> actual{};
      for (std::size_t iteration = 0;
           iteration < kIterations && !failed.load(std::memory_order_relaxed);
           ++iteration) {
        const auto offset =
            (thread_index * 997U + iteration * 131U) %
            (bytes.size() - actual.size() + 1U);
        const auto expected =
            std::span<const std::byte>{bytes}.subspan(offset, actual.size());
        if (opened.source->read_exact_at(offset, actual) !=
                MediaSourceError::none ||
            !std::equal(actual.begin(), actual.end(), expected.begin(),
                        expected.end())) {
          failed.store(true, std::memory_order_relaxed);
          break;
        }
      }
    });
  }
  for (auto& worker : workers) {
    worker.join();
  }
  return failed.load(std::memory_order_relaxed)
             ? fail("concurrent positional read consistency")
             : true;
}

}  // namespace

int main() {
  TemporaryDirectory temporary;
  if (!temporary.valid()) {
    std::cerr << "media source test failed: temporary directory creation\n";
    return 1;
  }
  if (!test_open_errors(temporary)) {
    return 1;
  }
  if (!test_final_link_rejection(temporary)) {
    return 1;
  }
#if defined(__linux__) || defined(__APPLE__)
  if (!test_posix_fifo_rejection(temporary) ||
      !test_posix_terminal_rejection()) {
    return 1;
  }
#endif
  return test_empty_file(temporary) &&
                 test_exact_and_boundary_reads(temporary) &&
                 test_path_replacement(temporary) &&
                 test_same_object_truncation(temporary) &&
                 test_same_object_rewrite(temporary) &&
                 test_shared_lifetime(temporary) &&
                 test_concurrent_positional_reads(temporary)
             ? 0
             : 1;
}

#endif
