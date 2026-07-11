#include "ohl/platform/media_source.hpp"
#include "ohl/vfs/udf_archive.hpp"

#include "../../src/vfs/src/udf_archive_internal.hpp"
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
#include <type_traits>
#include <utility>
#include <vector>

namespace {

constexpr const char* kSyntheticDirectory = "ohl-synthetic-fixture";
constexpr const char* kSyntheticFile = "sentinel.fixture";
constexpr auto kBlockSize = ohl::vfs::UdfArchiveLimits::logical_block_size;

using ohl::platform::MediaSourceError;
using ohl::vfs::DirectoryEntry;
using ohl::vfs::DirectoryPage;
using ohl::vfs::EntryType;
using ohl::vfs::UdfArchiveLimits;
using ohl::vfs::UdfDirectoryLimits;
using ohl::vfs::VfsError;
using ohl::vfs::detail::DirectoryArchiveTestHarness;
using ohl::vfs::detail::DirectoryEntryProvider;
using ohl::vfs::detail::DirectoryPageEngine;
using ohl::vfs::detail::DirectoryProviderFactory;
using ohl::vfs::detail::DirectoryProviderOpenResult;
using ohl::vfs::detail::DirectoryProviderResult;
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

[[nodiscard]] DirectoryProviderResult provider_entry(
    std::string name, const EntryType type = EntryType::file,
    const std::uint64_t size_bytes = 0) {
  return {
      .error = VfsError::none,
      .end = false,
      .entry = {.name = std::move(name),
                .type = type,
                .size_bytes = size_bytes},
  };
}

[[nodiscard]] DirectoryProviderResult provider_error(const VfsError error) {
  return {.error = error, .end = false, .entry = {}};
}

struct ProviderCounters {
  std::size_t opened{0};
  std::size_t destroyed{0};
  std::vector<std::string> paths;
};

class SequenceProvider final : public DirectoryEntryProvider {
 public:
  SequenceProvider(std::vector<DirectoryProviderResult> steps,
                   std::shared_ptr<ProviderCounters> counters)
      : steps_{std::move(steps)}, counters_{std::move(counters)} {}

  ~SequenceProvider() override {
    if (counters_ != nullptr) {
      ++counters_->destroyed;
    }
  }

  [[nodiscard]] DirectoryProviderResult next() override {
    if (next_ == steps_.size()) {
      return {.error = VfsError::none, .end = true, .entry = {}};
    }
    return steps_[next_++];
  }

 private:
  std::vector<DirectoryProviderResult> steps_;
  std::shared_ptr<ProviderCounters> counters_;
  std::size_t next_{0};
};

class SequenceFactory final : public DirectoryProviderFactory {
 public:
  explicit SequenceFactory(
      std::vector<DirectoryProviderResult> steps,
      const VfsError open_error = VfsError::none,
      std::shared_ptr<ProviderCounters> counters = {})
      : steps_{std::move(steps)},
        open_error_{open_error},
        counters_{counters == nullptr ? std::make_shared<ProviderCounters>()
                                      : std::move(counters)} {}

  [[nodiscard]] DirectoryProviderOpenResult open(
      const std::string_view normalized_path) override {
    ++counters_->opened;
    counters_->paths.emplace_back(normalized_path);
    if (open_error_ != VfsError::none) {
      return {.error = open_error_, .provider = {}};
    }
    return {
        .error = VfsError::none,
        .provider = std::make_unique<SequenceProvider>(steps_, counters_),
    };
  }

  [[nodiscard]] const std::shared_ptr<ProviderCounters>& counters() const {
    return counters_;
  }

 private:
  std::vector<DirectoryProviderResult> steps_;
  VfsError open_error_{VfsError::none};
  std::shared_ptr<ProviderCounters> counters_;
};

[[nodiscard]] std::unique_ptr<DirectoryEntryProvider> make_provider(
    std::vector<DirectoryProviderResult> steps,
    std::shared_ptr<ProviderCounters> counters = {}) {
  return std::make_unique<SequenceProvider>(std::move(steps),
                                            std::move(counters));
}

[[nodiscard]] bool expect_names(
    const std::vector<DirectoryEntry>& entries,
    const std::initializer_list<std::string_view> expected,
    const std::string_view context) {
  if (entries.size() != expected.size()) {
    return fail(context);
  }
  std::size_t index = 0;
  for (const auto name : expected) {
    if (entries[index++].name != name) {
      return fail(context);
    }
  }
  return true;
}

[[nodiscard]] bool error_page_is_empty(const DirectoryPage& page,
                                       const VfsError error) {
  return page.error == error && page.entries.empty() &&
         !page.cursor.valid() && !page.complete();
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

[[nodiscard]] bool test_directory_limit_validation() {
  static_assert(UdfDirectoryLimits::hard_max_path_components == 64);
  static_assert(UdfDirectoryLimits::hard_max_page_entries == 256);
  static_assert(UdfDirectoryLimits::hard_max_page_name_bytes == 64ULL * 1024ULL);
  static_assert(UdfDirectoryLimits::hard_max_page_result_bytes ==
                96ULL * 1024ULL);
  static_assert(UdfDirectoryLimits::hard_max_page_work == 1024);
  static_assert(UdfDirectoryLimits::hard_max_page_count == 64);
  static_assert(UdfDirectoryLimits::hard_max_cursor_work == 65'536);

  const UdfArchiveLimits defaults;
  if (!ohl::vfs::detail::valid_directory_limits(defaults)) {
    return fail("default directory limits");
  }

  auto invalid = [](const auto configure) {
    auto limits = UdfArchiveLimits{};
    configure(limits.directory);
    return !ohl::vfs::detail::valid_directory_limits(limits);
  };
  if (!invalid([](auto& value) { value.max_path_components = 0; }) ||
      !invalid([](auto& value) {
        value.max_path_components =
            UdfDirectoryLimits::hard_max_path_components + 1U;
      }) ||
      !invalid([](auto& value) { value.max_page_entries = 0; }) ||
      !invalid([](auto& value) {
        value.max_page_entries =
            UdfDirectoryLimits::hard_max_page_entries + 1U;
      }) ||
      !invalid([](auto& value) { value.max_page_name_bytes = 0; }) ||
      !invalid([](auto& value) {
        value.max_page_name_bytes =
            UdfDirectoryLimits::hard_max_page_name_bytes + 1U;
      }) ||
      !invalid([](auto& value) { value.max_page_result_bytes = 0; }) ||
      !invalid([](auto& value) {
        value.max_page_result_bytes =
            UdfDirectoryLimits::hard_max_page_result_bytes + 1U;
      }) ||
      !invalid([](auto& value) { value.max_page_work = 0; }) ||
      !invalid([](auto& value) {
        value.max_page_work = UdfDirectoryLimits::hard_max_page_work + 1U;
      }) ||
      !invalid([](auto& value) { value.max_page_count = 0; }) ||
      !invalid([](auto& value) {
        value.max_page_count = UdfDirectoryLimits::hard_max_page_count + 1U;
      }) ||
      !invalid([](auto& value) { value.max_cursor_work = 0; }) ||
      !invalid([](auto& value) {
        value.max_cursor_work =
            UdfDirectoryLimits::hard_max_cursor_work + 1U;
      })) {
    return fail("zero or raised directory limit");
  }

  auto depth_limits = UdfArchiveLimits{};
  depth_limits.directory.max_path_components = 2;
  return ohl::vfs::detail::path_within_depth("/one/two", depth_limits) &&
                 !ohl::vfs::detail::path_within_depth("/one/two/three",
                                                      depth_limits) &&
                 ohl::vfs::detail::path_within_depth("/", depth_limits)
             ? true
             : fail("exact and over path depth");
}

[[nodiscard]] bool test_bounded_directory_names() {
  const std::array<char, 1> sentinel{'s'};
  const auto impossible_size = ohl::vfs::detail::bounded_directory_name(
      {.data = sentinel.data(),
       .declared_size = std::numeric_limits<std::size_t>::max(),
       .terminated = true});
  const auto null_nonempty = ohl::vfs::detail::bounded_directory_name(
      {.data = nullptr, .declared_size = 1, .terminated = true});
  std::vector<char> exact(
      static_cast<std::size_t>(
          UdfDirectoryLimits::hard_max_page_name_bytes),
      'n');
  const auto exact_result = ohl::vfs::detail::bounded_directory_name(
      {.data = exact.data(),
       .declared_size = exact.size(),
       .terminated = true});
  exact.push_back('x');
  const auto over_result = ohl::vfs::detail::bounded_directory_name(
      {.data = exact.data(),
       .declared_size = exact.size(),
       .terminated = true});
  const std::array<char, 3> short_name{'a', 'b', 'c'};
  const auto nonterminated = ohl::vfs::detail::bounded_directory_name(
      {.data = short_name.data(),
       .declared_size = short_name.size(),
       .terminated = false});
  const std::array<char, 3> embedded{'a', '\0', 'b'};
  const auto embedded_nul = ohl::vfs::detail::bounded_directory_name(
      {.data = embedded.data(),
       .declared_size = embedded.size(),
       .terminated = true});
  return impossible_size.error == VfsError::limit_exceeded &&
                 impossible_size.name.empty() &&
                 null_nonempty.error == VfsError::read_failed &&
                 null_nonempty.name.empty() && exact_result.valid() &&
                 exact_result.name.size() ==
                     UdfDirectoryLimits::hard_max_page_name_bytes &&
                 over_result.error == VfsError::limit_exceeded &&
                 over_result.name.empty() &&
                 nonterminated.error == VfsError::read_failed &&
                 nonterminated.name.empty() &&
                 embedded_nul.error == VfsError::read_failed &&
                 embedded_nul.name.empty()
             ? true
             : fail("bounded raw directory names");
}

[[nodiscard]] bool test_entry_name_and_result_pages() {
  auto entry_limits = UdfArchiveLimits{};
  entry_limits.directory.max_page_entries = 2;
  DirectoryPageEngine exact_entries{
      make_provider({provider_entry("first"), provider_entry("second")}),
      entry_limits};
  const auto exact_entry_page = exact_entries.next_page();
  if (exact_entry_page.error != VfsError::none ||
      exact_entry_page.has_more ||
      !expect_names(exact_entry_page.entries, {"first", "second"},
                    "exact entry page")) {
    return false;
  }

  DirectoryPageEngine over_entries{
      make_provider({provider_entry("first"), provider_entry("second"),
                     provider_entry("third")}),
      entry_limits};
  const auto entry_page_one = over_entries.next_page();
  const auto entry_page_two = over_entries.next_page();
  if (entry_page_one.error != VfsError::none || !entry_page_one.has_more ||
      !expect_names(entry_page_one.entries, {"first", "second"},
                    "over-entry first page") ||
      entry_page_two.error != VfsError::none || entry_page_two.has_more ||
      !expect_names(entry_page_two.entries, {"third"},
                    "over-entry continuation")) {
    return false;
  }

  auto name_limits = UdfArchiveLimits{};
  name_limits.directory.max_page_name_bytes = 5;
  DirectoryPageEngine exact_names{
      make_provider({provider_entry("aa"), provider_entry("bbb")}),
      name_limits};
  const auto exact_name_page = exact_names.next_page();
  if (exact_name_page.error != VfsError::none || exact_name_page.has_more ||
      !expect_names(exact_name_page.entries, {"aa", "bbb"},
                    "exact name-byte page")) {
    return false;
  }
  DirectoryPageEngine over_names{
      make_provider({provider_entry("aa"), provider_entry("bbb"),
                     provider_entry("c")}),
      name_limits};
  const auto name_page_one = over_names.next_page();
  const auto name_page_two = over_names.next_page();
  if (!name_page_one.has_more ||
      !expect_names(name_page_one.entries, {"aa", "bbb"},
                    "over-name first page") ||
      name_page_two.error != VfsError::none || name_page_two.has_more ||
      !expect_names(name_page_two.entries, {"c"},
                    "over-name continuation")) {
    return false;
  }
  DirectoryPageEngine single_name_over{
      make_provider({provider_entry("123456")}), name_limits};
  const auto single_name_error = single_name_over.next_page();
  if (single_name_error.error != VfsError::limit_exceeded ||
      !single_name_error.entries.empty() || single_name_error.has_more) {
    return fail("single over-limit name was returned");
  }

  constexpr auto kMetadata =
      UdfDirectoryLimits::logical_entry_metadata_bytes;
  auto result_limits = UdfArchiveLimits{};
  result_limits.directory.max_page_result_bytes =
      (1U + kMetadata) + (2U + kMetadata);
  DirectoryPageEngine exact_results{
      make_provider({provider_entry("a"), provider_entry("bb")}),
      result_limits};
  const auto exact_result_page = exact_results.next_page();
  if (exact_result_page.error != VfsError::none ||
      exact_result_page.has_more ||
      !expect_names(exact_result_page.entries, {"a", "bb"},
                    "exact result-byte page")) {
    return false;
  }
  DirectoryPageEngine over_results{
      make_provider({provider_entry("a"), provider_entry("bb"),
                     provider_entry("c")}),
      result_limits};
  const auto result_page_one = over_results.next_page();
  const auto result_page_two = over_results.next_page();
  if (!result_page_one.has_more ||
      !expect_names(result_page_one.entries, {"a", "bb"},
                    "over-result first page") ||
      result_page_two.error != VfsError::none || result_page_two.has_more ||
      !expect_names(result_page_two.entries, {"c"},
                    "over-result continuation")) {
    return false;
  }
  result_limits.directory.max_page_result_bytes = kMetadata;
  DirectoryPageEngine single_result_over{
      make_provider({provider_entry("x")}), result_limits};
  const auto single_result_error = single_result_over.next_page();
  return single_result_error.error == VfsError::limit_exceeded &&
                 single_result_error.entries.empty() &&
                 !single_result_error.has_more
             ? true
             : fail("single over-limit result was returned");
}

[[nodiscard]] bool test_work_and_dot_limits() {
  auto limits = UdfArchiveLimits{};
  limits.directory.max_page_work = 3;
  DirectoryPageEngine exact{
      make_provider({provider_entry("."), provider_entry("visible")}), limits};
  const auto exact_page = exact.next_page();
  if (exact_page.error != VfsError::none || exact_page.has_more ||
      !expect_names(exact_page.entries, {"visible"}, "exact dot work")) {
    return false;
  }

  DirectoryPageEngine over{
      make_provider({provider_entry("."), provider_entry(".."),
                     provider_entry("visible")}),
      limits};
  const auto over_page = over.next_page();
  return over_page.error == VfsError::limit_exceeded &&
                 over_page.entries.empty() && !over_page.has_more
             ? true
             : fail("dot entries bypassed page work");
}

[[nodiscard]] bool test_page_and_cursor_work_limits() {
  auto page_limits = UdfArchiveLimits{};
  page_limits.directory.max_page_entries = 1;
  page_limits.directory.max_page_count = 2;
  DirectoryPageEngine exact_pages{
      make_provider({provider_entry("one"), provider_entry("two")}),
      page_limits};
  const auto exact_page_one = exact_pages.next_page();
  const auto exact_page_two = exact_pages.next_page();
  if (!exact_page_one.has_more || exact_page_two.error != VfsError::none ||
      exact_page_two.has_more ||
      !expect_names(exact_page_two.entries, {"two"}, "exact page count")) {
    return false;
  }
  DirectoryPageEngine over_pages{
      make_provider({provider_entry("one"), provider_entry("two"),
                     provider_entry("three")}),
      page_limits};
  (void)over_pages.next_page();
  (void)over_pages.next_page();
  const auto over_page_count = over_pages.next_page();
  if (over_page_count.error != VfsError::limit_exceeded ||
      !over_page_count.entries.empty() || over_page_count.has_more) {
    return fail("page-count ceiling did not fail closed");
  }

  auto work_limits = UdfArchiveLimits{};
  work_limits.directory.max_page_entries = 1;
  work_limits.directory.max_cursor_work = 3;
  DirectoryPageEngine exact_work{
      make_provider({provider_entry("one"), provider_entry("two")}),
      work_limits};
  const auto exact_work_one = exact_work.next_page();
  const auto exact_work_two = exact_work.next_page();
  if (!exact_work_one.has_more || exact_work_two.error != VfsError::none ||
      exact_work_two.has_more ||
      !expect_names(exact_work_two.entries, {"two"}, "exact cursor work")) {
    return false;
  }
  DirectoryPageEngine over_work{
      make_provider({provider_entry("one"), provider_entry("two"),
                     provider_entry("three")}),
      work_limits};
  (void)over_work.next_page();
  (void)over_work.next_page();
  const auto cursor_work_error = over_work.next_page();
  if (cursor_work_error.error != VfsError::limit_exceeded ||
      !cursor_work_error.entries.empty() || cursor_work_error.has_more) {
    return fail("cursor-work ceiling did not fail closed");
  }

  auto page_counter = std::make_shared<ProviderCounters>();
  auto page_factory = std::make_shared<SequenceFactory>(
      std::vector<DirectoryProviderResult>{provider_entry("one"),
                                           provider_entry("two"),
                                           provider_entry("three")},
      VfsError::none, page_counter);
  auto page_archive =
      DirectoryArchiveTestHarness::mount(page_factory, page_limits);
  auto public_page_one = page_archive.list_page("/");
  auto public_page_two =
      page_archive.continue_list(std::move(public_page_one.cursor));
  const auto public_page_error =
      page_archive.continue_list(std::move(public_page_two.cursor));
  if (!error_page_is_empty(public_page_error, VfsError::limit_exceeded) ||
      page_counter->destroyed != 1) {
    return fail("page-count termination did not release provider");
  }

  auto work_counter = std::make_shared<ProviderCounters>();
  auto work_factory = std::make_shared<SequenceFactory>(
      std::vector<DirectoryProviderResult>{provider_entry("one"),
                                           provider_entry("two"),
                                           provider_entry("three")},
      VfsError::none, work_counter);
  auto work_archive =
      DirectoryArchiveTestHarness::mount(work_factory, work_limits);
  auto public_work_one = work_archive.list_page("/");
  auto public_work_two =
      work_archive.continue_list(std::move(public_work_one.cursor));
  const auto public_work_error =
      work_archive.continue_list(std::move(public_work_two.cursor));
  return error_page_is_empty(public_work_error, VfsError::limit_exceeded) &&
                 work_counter->destroyed == 1
             ? true
             : fail("cursor-work termination did not release provider");
}

[[nodiscard]] bool test_empty_and_exact_full_eof() {
  DirectoryPageEngine empty{make_provider({}), {}};
  const auto empty_page = empty.next_page();
  if (empty_page.error != VfsError::none || !empty_page.entries.empty() ||
      empty_page.has_more) {
    return fail("empty directory page");
  }

  auto limits = UdfArchiveLimits{};
  limits.directory.max_page_entries = 2;
  DirectoryPageEngine exact_full{
      make_provider({provider_entry("one"), provider_entry("two")}), limits};
  const auto full_page = exact_full.next_page();
  return full_page.error == VfsError::none && !full_page.has_more &&
                 expect_names(full_page.entries, {"one", "two"},
                              "exact-full EOF")
             ? true
             : false;
}

[[nodiscard]] UdfArchiveLimits small_page_limits() {
  auto limits = UdfArchiveLimits{};
  limits.directory.max_page_entries = 2;
  return limits;
}

[[nodiscard]] bool append_page_names(const DirectoryPage& page,
                                     std::vector<std::string>& names) {
  if (page.error != VfsError::none) {
    return false;
  }
  for (const auto& entry : page.entries) {
    names.push_back(entry.name);
  }
  return true;
}

[[nodiscard]] bool test_public_ordered_pagination() {
  auto factory = std::make_shared<SequenceFactory>(
      std::vector<DirectoryProviderResult>{
          provider_entry("zero"), provider_entry("one"),
          provider_entry("two"), provider_entry("three"),
          provider_entry("four")});
  auto archive =
      DirectoryArchiveTestHarness::mount(factory, small_page_limits());
  if (!archive.is_open()) {
    return fail("synthetic pagination mount");
  }

  std::vector<std::string> names;
  auto page = archive.list_page("//ordered");
  const std::array<std::size_t, 3> expected_page_sizes{2, 2, 1};
  std::size_t page_index = 0;
  while (true) {
    if (page_index == expected_page_sizes.size() ||
        page.entries.size() != expected_page_sizes[page_index] ||
        !append_page_names(page, names)) {
      return fail("ordered page shape");
    }
    ++page_index;
    if (page.complete()) {
      break;
    }
    if (!page.cursor.valid()) {
      return fail("ordered continuation cursor");
    }
    page = archive.continue_list(std::move(page.cursor));
  }
  if (page_index != expected_page_sizes.size() ||
      names != std::vector<std::string>{"zero", "one", "two", "three",
                                        "four"} ||
      factory->counters()->opened != 1 ||
      factory->counters()->destroyed != 1 ||
      factory->counters()->paths != std::vector<std::string>{"/ordered"}) {
    return fail("ordered pagination result or normal-exhaustion cleanup");
  }

  std::vector<std::string> repeated_names;
  auto repeated = archive.list_page("/ordered");
  while (true) {
    if (!append_page_names(repeated, repeated_names)) {
      return fail("repeated pagination error");
    }
    if (repeated.complete()) {
      break;
    }
    repeated = archive.continue_list(std::move(repeated.cursor));
  }
  return repeated_names == names && factory->counters()->opened == 2 &&
                 factory->counters()->destroyed == 2
             ? true
             : fail("pagination repeat or normal-exhaustion cleanup");
}

[[nodiscard]] std::shared_ptr<SequenceFactory> two_entry_factory() {
  return std::make_shared<SequenceFactory>(
      std::vector<DirectoryProviderResult>{provider_entry("first"),
                                           provider_entry("second")});
}

[[nodiscard]] UdfArchiveLimits one_entry_page_limits() {
  auto limits = UdfArchiveLimits{};
  limits.directory.max_page_entries = 1;
  return limits;
}

[[nodiscard]] bool test_public_cursor_contract() {
  static_assert(std::is_default_constructible_v<ohl::vfs::DirectoryCursor>);
  static_assert(!std::is_copy_constructible_v<ohl::vfs::DirectoryCursor>);
  static_assert(!std::is_copy_assignable_v<ohl::vfs::DirectoryCursor>);
  static_assert(
      std::is_nothrow_move_constructible_v<ohl::vfs::DirectoryCursor>);
  static_assert(std::is_nothrow_move_assignable_v<ohl::vfs::DirectoryCursor>);
  static_assert(std::is_default_constructible_v<DirectoryPage>);
  static_assert(!std::is_copy_constructible_v<DirectoryPage>);
  static_assert(std::is_nothrow_move_constructible_v<DirectoryPage>);

  const auto limits = one_entry_page_limits();
  auto archive = DirectoryArchiveTestHarness::mount(two_entry_factory(), limits);
  auto page = archive.list_page("/");
  if (page.error != VfsError::none || !page.cursor.valid() ||
      !expect_names(page.entries, {"first"}, "cursor first page")) {
    return false;
  }
  auto cursor = std::move(page.cursor);
  auto moved = std::move(cursor);
  if (cursor.valid() || !moved.valid()) {
    return fail("cursor move state");
  }
  const auto moved_from = archive.continue_list(std::move(cursor));
  if (!error_page_is_empty(moved_from, VfsError::invalid_cursor)) {
    return fail("moved-from cursor rejection");
  }
  const auto moved_result = archive.continue_list(std::move(moved));
  if (moved_result.error != VfsError::none || !moved_result.complete() ||
      !expect_names(moved_result.entries, {"second"},
                    "moved cursor continuation")) {
    return false;
  }
  const auto reused = archive.continue_list(std::move(moved));
  if (!error_page_is_empty(reused, VfsError::invalid_cursor)) {
    return fail("consumed cursor reuse rejection");
  }

  auto shared_page = archive.list_page("/");
  auto shared = archive.share();
  const auto shared_result =
      shared.continue_list(std::move(shared_page.cursor));
  if (shared_result.error != VfsError::none || !shared_result.complete() ||
      !expect_names(shared_result.entries, {"second"},
                    "same-state shared cursor")) {
    return false;
  }

  auto foreign_page = archive.list_page("/");
  auto foreign =
      DirectoryArchiveTestHarness::mount(two_entry_factory(), limits);
  const auto foreign_result =
      foreign.continue_list(std::move(foreign_page.cursor));
  if (!error_page_is_empty(foreign_result, VfsError::invalid_cursor)) {
    return fail("foreign cursor rejection");
  }

  auto stale_page = archive.list_page("/");
  archive.close();
  const auto stale_result =
      archive.continue_list(std::move(stale_page.cursor));
  if (!error_page_is_empty(stale_result, VfsError::invalid_cursor)) {
    return fail("closed archive stale cursor rejection");
  }

  archive = DirectoryArchiveTestHarness::mount(two_entry_factory(), limits);
  auto reopened_page = archive.list_page("/");
  auto old_cursor = std::move(reopened_page.cursor);
  archive = DirectoryArchiveTestHarness::mount(two_entry_factory(), limits);
  const auto reopened_result = archive.continue_list(std::move(old_cursor));
  if (!error_page_is_empty(reopened_result, VfsError::invalid_cursor)) {
    return fail("reopened archive stale cursor rejection");
  }

  ohl::vfs::DirectoryCursor invalid;
  const auto invalid_result = archive.continue_list(std::move(invalid));
  return error_page_is_empty(invalid_result, VfsError::invalid_cursor)
             ? true
             : fail("default invalid cursor rejection");
}

[[nodiscard]] bool test_abandoned_cursor_lifetime() {
  auto retained = std::make_shared<int>(42);
  std::weak_ptr<int> lifetime{retained};
  auto counters = std::make_shared<ProviderCounters>();
  auto factory = std::make_shared<SequenceFactory>(
      std::vector<DirectoryProviderResult>{provider_entry("first"),
                                           provider_entry("second")},
      VfsError::none, counters);
  auto archive = DirectoryArchiveTestHarness::mount(
      factory, one_entry_page_limits(), retained);
  retained.reset();
  auto page = archive.list_page("/");
  if (!page.cursor.valid() || lifetime.expired() || counters->destroyed != 0) {
    return fail("cursor did not retain synthetic mount lifetime");
  }
  archive.close();
  if (lifetime.expired() || counters->destroyed != 0) {
    return fail("archive close released live cursor state");
  }
  page = DirectoryPage{};
  return lifetime.expired() && counters->destroyed == 1
             ? true
             : fail("abandoned cursor did not release retained lifetime");
}

[[nodiscard]] bool test_public_depth_and_open_errors() {
  auto limits = UdfArchiveLimits{};
  limits.directory.max_path_components = 2;
  auto factory = std::make_shared<SequenceFactory>(
      std::vector<DirectoryProviderResult>{provider_entry("entry")});
  auto archive = DirectoryArchiveTestHarness::mount(factory, limits);
  const auto exact = archive.list_page("/one/two");
  const auto over = archive.list_page("/one/two/three");
  if (exact.error != VfsError::none || !exact.complete() ||
      !expect_names(exact.entries, {"entry"}, "public exact depth") ||
      !error_page_is_empty(over, VfsError::limit_exceeded) ||
      factory->counters()->opened != 1) {
    return fail("public depth limit");
  }

  auto missing_factory = std::make_shared<SequenceFactory>(
      std::vector<DirectoryProviderResult>{}, VfsError::not_found);
  auto missing = DirectoryArchiveTestHarness::mount(missing_factory, limits);
  const auto missing_page = missing.list_page("/");
  return error_page_is_empty(missing_page, VfsError::not_found)
             ? true
             : fail("provider open error mapping");
}

[[nodiscard]] bool test_injected_errors_are_tokenless() {
  for (const auto error : {VfsError::source_changed, VfsError::read_failed,
                           VfsError::limit_exceeded}) {
    auto counters = std::make_shared<ProviderCounters>();
    auto factory = std::make_shared<SequenceFactory>(
        std::vector<DirectoryProviderResult>{provider_entry("partial"),
                                             provider_error(error)},
        VfsError::none, counters);
    auto archive = DirectoryArchiveTestHarness::mount(factory);
    const auto page = archive.list_page("/");
    if (!error_page_is_empty(page, error) || counters->destroyed != 1) {
      return fail("injected first-page error output or provider cleanup");
    }
  }

  auto counters = std::make_shared<ProviderCounters>();
  auto factory = std::make_shared<SequenceFactory>(
      std::vector<DirectoryProviderResult>{
          provider_entry("first"), provider_entry("pending"),
          provider_error(VfsError::source_changed)},
      VfsError::none, counters);
  auto archive = DirectoryArchiveTestHarness::mount(
      factory, one_entry_page_limits());
  auto first = archive.list_page("/");
  if (first.error != VfsError::none || !first.cursor.valid() ||
      !expect_names(first.entries, {"first"}, "pre-mutation page")) {
    return false;
  }
  const auto changed = archive.continue_list(std::move(first.cursor));
  return error_page_is_empty(changed, VfsError::source_changed) &&
                 counters->destroyed == 1
             ? true
             : fail("continuation mutation output, cursor, or cleanup");
}

[[nodiscard]] bool test_legacy_complete_or_error() {
  auto factory = std::make_shared<SequenceFactory>(
      std::vector<DirectoryProviderResult>{provider_entry("one"),
                                           provider_entry("two"),
                                           provider_entry("three")});
  auto archive = DirectoryArchiveTestHarness::mount(
      factory, one_entry_page_limits());
  const auto complete = archive.list("/");
  if (complete.error != VfsError::none ||
      !expect_names(complete.entries, {"one", "two", "three"},
                    "legacy complete listing")) {
    return false;
  }

  auto error_factory = std::make_shared<SequenceFactory>(
      std::vector<DirectoryProviderResult>{provider_entry("partial"),
                                           provider_error(VfsError::read_failed)});
  auto error_archive = DirectoryArchiveTestHarness::mount(error_factory);
  const auto failed = error_archive.list("/");
  if (failed.error != VfsError::read_failed || !failed.entries.empty()) {
    return fail("legacy listing exposed partial error output");
  }

  auto ceiling_limits = one_entry_page_limits();
  ceiling_limits.directory.max_page_count = 2;
  auto ceiling_archive = DirectoryArchiveTestHarness::mount(
      factory, ceiling_limits);
  const auto ceiling = ceiling_archive.list("/");
  return ceiling.error == VfsError::limit_exceeded &&
                 ceiling.entries.empty()
             ? true
             : fail("legacy listing did not fail closed at cursor ceiling");
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
      !test_size_validation() || !test_directory_limit_validation() ||
      !test_bounded_directory_names() ||
      !test_entry_name_and_result_pages() || !test_work_and_dot_limits() ||
      !test_page_and_cursor_work_limits() ||
      !test_empty_and_exact_full_eof() ||
      !test_public_ordered_pagination() || !test_public_cursor_contract() ||
      !test_abandoned_cursor_lifetime() ||
      !test_public_depth_and_open_errors() ||
      !test_injected_errors_are_tokenless() ||
      !test_legacy_complete_or_error()) {
    std::cerr << "synthetic path or size validation failed\n";
    return 1;
  }

  ohl::vfs::UdfArchive archive;
  const auto missing = open_source(
      std::filesystem::path{"ohl-synthetic-missing-image.fixture"});
  ohl::vfs::DirectoryCursor invalid_cursor;
  if (archive.is_open() || archive.share().is_open() ||
      archive.list("/").error != VfsError::not_open ||
      !error_page_is_empty(archive.list_page("/"), VfsError::not_open) ||
      !error_page_is_empty(
          archive.continue_list(std::move(invalid_cursor)),
          VfsError::invalid_cursor) ||
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
