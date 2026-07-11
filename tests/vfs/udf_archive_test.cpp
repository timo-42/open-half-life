#include "ohl/platform/media_source.hpp"
#include "ohl/vfs/udf_archive.hpp"

#include "../../src/vfs/src/udf_media_input.hpp"

#include <algorithm>
#include <array>
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
#include <utility>
#include <vector>

namespace {

constexpr const char* kSyntheticDirectory = "ohl-synthetic-fixture";
constexpr const char* kSyntheticFile = "sentinel.fixture";
constexpr auto kBlockSize = ohl::vfs::UdfArchiveLimits::logical_block_size;

using ohl::platform::MediaSourceError;
using ohl::vfs::UdfArchiveLimits;
using ohl::vfs::VfsError;
using ohl::vfs::detail::UdfMediaInputHandle;

class TemporaryDirectory final {
 public:
  TemporaryDirectory() {
    std::error_code error;
    const auto parent = std::filesystem::temp_directory_path(error);
    if (error) {
      return;
    }
    const auto nonce =
        std::chrono::steady_clock::now().time_since_epoch().count();
    for (unsigned int attempt = 0; attempt < 128; ++attempt) {
      auto candidate = parent / ("ohl-udf-adapter-test-" +
                                 std::to_string(nonce) + "-" +
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

[[nodiscard]] bool fail(const std::string_view message) {
  std::cerr << "UDF archive test failed: " << message << '\n';
  return false;
}

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

[[nodiscard]] bool expect_bytes(const std::span<const std::byte> actual,
                                const std::span<const std::byte> expected,
                                const std::string_view context) {
  return actual.size() == expected.size() &&
                 std::equal(actual.begin(), actual.end(), expected.begin())
             ? true
             : fail(context);
}

int expect_path(const std::string& input, const std::string& expected) {
  const auto actual = ohl::vfs::normalize_path(input);
  if (!actual.has_value() || *actual != expected) {
    std::cerr << "unexpected normalized synthetic path\n";
    return 1;
  }
  return 0;
}

int expect_rejected(const std::string& input) {
  if (ohl::vfs::normalize_path(input).has_value()) {
    std::cerr << "unsafe synthetic path was accepted\n";
    return 1;
  }
  return 0;
}

[[nodiscard]] bool test_size_validation() {
  std::uint32_t blocks = 99;
  const UdfArchiveLimits defaults;
  if (ohl::vfs::detail::validate_udf_media_source_size(0, defaults, blocks) !=
          VfsError::invalid_source ||
      blocks != 0) {
    return fail("empty source validation");
  }
  if (ohl::vfs::detail::validate_udf_media_source_size(kBlockSize + 1,
                                                        defaults, blocks) !=
          VfsError::invalid_source ||
      blocks != 0) {
    return fail("partial-block source validation");
  }
  if (ohl::vfs::detail::validate_udf_media_source_size(
          UdfArchiveLimits::max_representable_source_bytes, defaults,
          blocks) != VfsError::none ||
      blocks != std::numeric_limits<std::uint32_t>::max()) {
    return fail("maximum block-count conversion");
  }
  if (ohl::vfs::detail::validate_udf_media_source_size(
          UdfArchiveLimits::max_representable_source_bytes + kBlockSize,
          defaults, blocks) != VfsError::limit_exceeded ||
      blocks != 0) {
    return fail("overflowing block-count conversion");
  }

  auto limited = defaults;
  limited.max_source_bytes = kBlockSize;
  return ohl::vfs::detail::validate_udf_media_source_size(
             kBlockSize * 2, limited, blocks) == VfsError::limit_exceeded &&
                 blocks == 0
             ? true
             : fail("configured source-size limit");
}

#if !defined(OHL_PLATFORM_MEDIA_SOURCE_UNSUPPORTED)

[[nodiscard]] ohl::platform::MediaSourceOpenResult open_source(
    const std::filesystem::path& path) {
  return ohl::platform::open_media_source(path);
}

[[nodiscard]] bool test_exact_block_reads(
    const TemporaryDirectory& temporary) {
  const auto path = temporary.path() / "block-boundaries.fixture";
  const auto bytes = make_bytes(static_cast<std::size_t>(kBlockSize * 4), 17U);
  if (!write_bytes(path, bytes)) {
    return fail("block-boundary fixture creation");
  }
  auto opened = open_source(path);
  auto input = ohl::vfs::detail::create_udf_media_input(opened.source, {});
  if (!opened.valid() || !input.valid() ||
      ohl::vfs::detail::udf_media_input_size_blocks(input.input.get()) != 4) {
    return fail("block-boundary adapter creation");
  }

  std::vector<std::byte> first(static_cast<std::size_t>(kBlockSize));
  std::vector<std::byte> middle(static_cast<std::size_t>(kBlockSize * 2));
  std::vector<std::byte> last(static_cast<std::size_t>(kBlockSize));
  if (ohl::vfs::detail::udf_media_input_read_blocks(
          input.input.get(), 0, first.data(), 1) != 1 ||
      ohl::vfs::detail::udf_media_input_read_blocks(
          input.input.get(), 1, middle.data(), 2) != 2 ||
      ohl::vfs::detail::udf_media_input_read_blocks(
          input.input.get(), 3, last.data(), 1) != 1 ||
      ohl::vfs::detail::udf_media_input_read_blocks(input.input.get(), 4,
                                                     nullptr, 0) != 0) {
    return fail("exact block reads");
  }
  const auto source_bytes = std::span<const std::byte>{bytes};
  return expect_bytes(first, source_bytes.first(first.size()),
                      "first block bytes") &&
         expect_bytes(middle, source_bytes.subspan(first.size(), middle.size()),
                      "middle block bytes") &&
         expect_bytes(last, source_bytes.last(last.size()),
                      "last block bytes") &&
         ohl::vfs::detail::udf_media_input_source_error(input.input.get()) ==
             MediaSourceError::none;
}

[[nodiscard]] UdfMediaInputHandle make_input(
    const ohl::vfs::SharedMediaSource& source,
    const UdfArchiveLimits limits = {}) {
  return ohl::vfs::detail::create_udf_media_input(source, limits).input;
}

[[nodiscard]] bool test_rejected_block_reads(
    const TemporaryDirectory& temporary) {
  const auto path = temporary.path() / "rejected-ranges.fixture";
  const auto bytes = make_bytes(static_cast<std::size_t>(kBlockSize * 4), 29U);
  if (!write_bytes(path, bytes)) {
    return fail("rejected-range fixture creation");
  }
  const auto opened = open_source(path);
  if (!opened.valid()) {
    return fail("rejected-range source acquisition");
  }
  std::array<std::byte, static_cast<std::size_t>(kBlockSize * 2)> buffer{};

  auto out_of_range = make_input(opened.source);
  if (ohl::vfs::detail::udf_media_input_read_blocks(
          out_of_range.get(), 4, buffer.data(), 1) != -1 ||
      ohl::vfs::detail::udf_media_input_source_error(out_of_range.get()) !=
          MediaSourceError::out_of_range) {
    return fail("read beyond final block");
  }

  auto overflow = make_input(opened.source);
  if (ohl::vfs::detail::udf_media_input_read_blocks(
          overflow.get(), std::numeric_limits<std::uint32_t>::max(),
          buffer.data(), std::numeric_limits<std::uint32_t>::max()) != -1 ||
      ohl::vfs::detail::udf_media_input_source_error(overflow.get()) !=
          MediaSourceError::out_of_range) {
    return fail("overflowing block request");
  }

  auto null_destination = make_input(opened.source);
  if (ohl::vfs::detail::udf_media_input_read_blocks(
          null_destination.get(), 0, nullptr, 1) != -1 ||
      ohl::vfs::detail::udf_media_input_source_error(
          null_destination.get()) != MediaSourceError::out_of_range) {
    return fail("null block destination");
  }

  auto limited_config = UdfArchiveLimits{};
  limited_config.max_blocks_per_read = 1;
  auto limited = make_input(opened.source, limited_config);
  return ohl::vfs::detail::udf_media_input_read_blocks(
             limited.get(), 0, buffer.data(), 2) == -1 &&
                 ohl::vfs::detail::udf_media_input_source_error(
                     limited.get()) == MediaSourceError::out_of_range
             ? true
             : fail("configured block-read limit");
}

[[nodiscard]] bool test_source_creation_limits(
    const TemporaryDirectory& temporary) {
  const auto partial_path = temporary.path() / "partial-block.fixture";
  const auto partial = make_bytes(static_cast<std::size_t>(kBlockSize + 1), 31U);
  const auto full_path = temporary.path() / "limited-source.fixture";
  const auto full = make_bytes(static_cast<std::size_t>(kBlockSize * 2), 37U);
  if (!write_bytes(partial_path, partial) || !write_bytes(full_path, full)) {
    return fail("source-limit fixture creation");
  }
  const auto partial_source = open_source(partial_path);
  const auto full_source = open_source(full_path);
  if (!partial_source.valid() || !full_source.valid()) {
    return fail("source-limit fixture acquisition");
  }

  auto partial_input =
      ohl::vfs::detail::create_udf_media_input(partial_source.source, {});
  auto byte_limited = UdfArchiveLimits{};
  byte_limited.max_source_bytes = kBlockSize;
  auto limited_input = ohl::vfs::detail::create_udf_media_input(
      full_source.source, byte_limited);
  auto invalid_read_limit = UdfArchiveLimits{};
  invalid_read_limit.max_blocks_per_read = 0;
  auto invalid_limit_input = ohl::vfs::detail::create_udf_media_input(
      full_source.source, invalid_read_limit);
  return !partial_input.valid() &&
                 partial_input.error == VfsError::invalid_source &&
                 !limited_input.valid() &&
                 limited_input.error == VfsError::limit_exceeded &&
                 !invalid_limit_input.valid() &&
                 invalid_limit_input.error == VfsError::limit_exceeded
             ? true
             : fail("adapter source and configuration limits");
}

[[nodiscard]] bool test_failed_open_cleanup(
    const TemporaryDirectory& temporary) {
  const auto path = temporary.path() / "invalid-udf.fixture";
  const auto bytes =
      make_bytes(static_cast<std::size_t>(kBlockSize * 512), 43U);
  if (!write_bytes(path, bytes)) {
    return fail("failed-open fixture creation");
  }
  auto opened = open_source(path);
  if (!opened.valid()) {
    return fail("failed-open source acquisition");
  }
  std::weak_ptr<const ohl::platform::MediaSource> lifetime{opened.source};
  ohl::vfs::UdfArchive archive;
  const auto error = archive.open(std::move(opened.source));
  if (error != VfsError::open_failed || archive.is_open()) {
    return fail("invalid UDF open result");
  }
  if (!lifetime.expired()) {
    return fail("failed-open adapter lifetime release");
  }
  if (archive.open(ohl::vfs::SharedMediaSource{}) !=
      VfsError::invalid_source) {
    return fail("null capability rejection");
  }
  return true;
}

[[nodiscard]] bool test_path_replacement_pinning(
    const TemporaryDirectory& temporary) {
  const auto selected = temporary.path() / "selected-source.fixture";
  const auto original_path = temporary.path() / "original-source.fixture";
  const auto original =
      make_bytes(static_cast<std::size_t>(kBlockSize * 2), 47U);
  const auto replacement =
      make_bytes(static_cast<std::size_t>(kBlockSize * 2), 53U);
  if (!write_bytes(selected, original)) {
    return fail("replacement fixture creation");
  }
  const auto opened = open_source(selected);
  auto input = ohl::vfs::detail::create_udf_media_input(opened.source, {});
  if (!opened.valid() || !input.valid()) {
    return fail("replacement adapter creation");
  }

  std::error_code error;
  std::filesystem::rename(selected, original_path, error);
  if (error || !write_bytes(selected, replacement)) {
    return fail("replacement setup");
  }
  std::vector<std::byte> actual(original.size());
  return ohl::vfs::detail::udf_media_input_read_blocks(
             input.input.get(), 0, actual.data(), 2) == 2 &&
                 expect_bytes(actual, original,
                              "adapter retargeted after path replacement")
             ? true
             : fail("pinned replacement read");
}

[[nodiscard]] bool test_mutation_detection(
    const TemporaryDirectory& temporary) {
  const auto path = temporary.path() / "mutated-source.fixture";
  const auto original =
      make_bytes(static_cast<std::size_t>(kBlockSize * 2), 59U);
  const auto truncated =
      make_bytes(static_cast<std::size_t>(kBlockSize), 61U);
  if (!write_bytes(path, original)) {
    return fail("mutation fixture creation");
  }
  const auto opened = open_source(path);
  auto input = ohl::vfs::detail::create_udf_media_input(opened.source, {});
  if (!opened.valid() || !input.valid() || !write_bytes(path, truncated)) {
    return fail("mutation setup");
  }
  std::vector<std::byte> destination(static_cast<std::size_t>(kBlockSize));
  return ohl::vfs::detail::udf_media_input_read_blocks(
             input.input.get(), 0, destination.data(), 1) == -1 &&
                 ohl::vfs::detail::udf_media_input_source_error(
                     input.input.get()) == MediaSourceError::changed
             ? true
             : fail("same-object mutation detection");
}

#endif

}  // namespace

int main(const int argument_count, const char* const arguments[]) {
  const std::string relative_path =
      std::string{kSyntheticDirectory} + '/' + kSyntheticFile;
  const std::string absolute_path = '/' + relative_path;
  const std::string mixed_separator_path =
      std::string{"//"} + kSyntheticDirectory + '\\' + kSyntheticFile;
  const std::string traversal_path = std::string{"../"} + kSyntheticFile;
  const std::string dotted_path =
      std::string{kSyntheticDirectory} + "/./" + kSyntheticFile;
  const std::string parent_path =
      std::string{kSyntheticDirectory} + "/../" + kSyntheticFile;

  if (expect_path("", "/") != 0 || expect_path("/", "/") != 0 ||
      expect_path(relative_path, absolute_path) != 0 ||
      expect_path(mixed_separator_path, absolute_path) != 0 ||
      expect_rejected(traversal_path) != 0 ||
      expect_rejected(dotted_path) != 0 ||
      expect_rejected(parent_path) != 0 ||
      expect_rejected(std::string{"safe\0hidden", 11}) != 0 ||
      !ohl::vfs::is_single_path_component(kSyntheticFile) ||
      ohl::vfs::is_single_path_component(traversal_path) ||
      ohl::vfs::is_single_path_component(relative_path) ||
      ohl::vfs::is_single_path_component(mixed_separator_path) ||
      ohl::vfs::is_single_path_component(std::string{"safe\0hidden", 11}) ||
      !test_size_validation()) {
    std::cerr << "synthetic path or size validation failed\n";
    return 1;
  }

  ohl::vfs::UdfArchive archive;
  const auto missing = open_source(
      std::filesystem::path{"ohl-synthetic-missing-image.fixture"});
  if (archive.is_open() || archive.share().is_open() ||
      archive.list("/").error != VfsError::not_open ||
      missing.valid() ||
      archive.open(missing.source) != VfsError::invalid_source ||
      archive.open_file(kSyntheticFile) != nullptr) {
    std::cerr << "closed archive contract failed\n";
    return 1;
  }

#if !defined(OHL_PLATFORM_MEDIA_SOURCE_UNSUPPORTED)
  const TemporaryDirectory temporary;
  if (!temporary.valid() || !test_exact_block_reads(temporary) ||
      !test_rejected_block_reads(temporary) ||
      !test_source_creation_limits(temporary) ||
      !test_failed_open_cleanup(temporary) ||
      !test_path_replacement_pinning(temporary) ||
      !test_mutation_detection(temporary)) {
    return 1;
  }
#endif

  if (argument_count == 2) {
    auto opened = open_source(std::filesystem::path{arguments[1]});
    if (!opened.valid() ||
        archive.open(std::move(opened.source)) != VfsError::none) {
      std::cerr << "runtime integration image did not mount\n";
      return 1;
    }
    const auto root = archive.list("/");
    if (root.error != VfsError::none || root.entries.empty()) {
      std::cerr << "runtime integration root could not be listed\n";
      return 1;
    }
    for (const auto& entry : root.entries) {
      if (entry.type == ohl::vfs::EntryType::file && entry.size_bytes > 0) {
        auto file = archive.open_file_at("/", entry.name);
        std::array<std::byte, 1> byte{};
        if (!file || file->size() != entry.size_bytes) {
          std::cerr << "runtime integration file could not be read\n";
          return 1;
        }
        archive.close();
        if (file->read(byte) != 1) {
          std::cerr << "open file did not retain its archive lifetime\n";
          return 1;
        }
        return 0;
      }
    }
    std::cerr << "runtime integration image had no non-empty root file\n";
    return 1;
  }
  return 0;
}
