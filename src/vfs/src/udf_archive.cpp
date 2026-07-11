#include "ohl/vfs/udf_archive.hpp"

#include "ohl/platform/media_source.hpp"
#include "udf_archive_internal.hpp"
#include "udf_media_input.hpp"

#if defined(_MSC_VER)
#include <BaseTsd.h>
typedef SSIZE_T ssize_t;
#endif
#include <udfread/udfread.h>

#include <algorithm>
#include <iterator>
#include <limits>
#include <mutex>
#include <optional>
#include <utility>

namespace ohl::vfs {
namespace {

[[nodiscard]] EntryType entry_type(const unsigned int udf_type) noexcept {
  switch (udf_type) {
    case UDF_DT_REG:
      return EntryType::file;
    case UDF_DT_DIR:
      return EntryType::directory;
    default:
      return EntryType::unknown;
  }
}

struct DirectoryCloser {
  void operator()(UDFDIR* directory) const noexcept {
    udfread_closedir(directory);
  }
};

using DirectoryHandle = std::unique_ptr<UDFDIR, DirectoryCloser>;

struct FileCloser {
  void operator()(UDFFILE* file) const noexcept {
    udfread_file_close(file);
  }
};

using FileHandle = std::unique_ptr<UDFFILE, FileCloser>;

struct ArchiveState {
  ArchiveState(SharedMediaSource input_source,
               const UdfArchiveLimits input_limits)
      : source{std::move(input_source)}, limits{input_limits} {}

  udfread* archive{nullptr};
  SharedMediaSource source;
  const UdfArchiveLimits limits;
  std::shared_ptr<detail::DirectoryProviderFactory> synthetic_factory;
  std::shared_ptr<void> synthetic_lifetime;
  mutable std::recursive_mutex mutex;

  ~ArchiveState() {
    if (archive != nullptr) {
      udfread_close(archive);
    }
  }
};

class UdfDirectoryProvider final : public detail::DirectoryEntryProvider {
 public:
  UdfDirectoryProvider(std::shared_ptr<ArchiveState> input_owner,
                       UDFDIR* input_directory)
      : owner_{std::move(input_owner)}, directory_{input_directory} {}

  ~UdfDirectoryProvider() override {
    const std::scoped_lock lock{owner_->mutex};
    directory_.reset();
  }

  [[nodiscard]] detail::DirectoryProviderResult next() override {
    const std::scoped_lock lock{owner_->mutex};
    udfread_dirent entry{};
    if (udfread_readdir(directory_.get(), &entry) == nullptr) {
      return {.error = VfsError::none, .end = true, .entry = {}};
    }
    if (entry.d_name == nullptr) {
      return {.error = VfsError::read_failed, .end = false, .entry = {}};
    }

    std::size_t name_size = 0;
    constexpr auto kMaximumNameBytes = static_cast<std::size_t>(
        UdfDirectoryLimits::hard_max_page_name_bytes);
    constexpr auto kMaximumProbeBytes = kMaximumNameBytes + 2U;
    while (name_size < kMaximumProbeBytes &&
           entry.d_name[name_size] != '\0') {
      ++name_size;
    }
    auto name = detail::bounded_directory_name({
        .data = entry.d_name,
        .declared_size = name_size,
        .terminated = name_size < kMaximumProbeBytes,
    });
    if (!name.valid()) {
      return {.error = name.error, .end = false, .entry = {}};
    }

    DirectoryEntry item{
        .name = std::move(name.name),
        .type = entry_type(entry.d_type),
        .size_bytes = 0,
    };
    if (entry.d_type == UDF_DT_REG) {
      FileHandle file{udfread_file_openat(directory_.get(), entry.d_name)};
      if (!file) {
        return {.error = VfsError::read_failed, .end = false, .entry = {}};
      }
      const auto size = udfread_file_size(file.get());
      if (size < 0) {
        return {.error = VfsError::read_failed, .end = false, .entry = {}};
      }
      item.size_bytes = static_cast<std::uint64_t>(size);
    }
    return {
        .error = VfsError::none, .end = false, .entry = std::move(item)};
  }

 private:
  std::shared_ptr<ArchiveState> owner_;
  DirectoryHandle directory_;
};

[[nodiscard]] VfsError map_open_input_error(
    const platform::MediaSourceError error) noexcept {
  switch (error) {
    case platform::MediaSourceError::none:
    case platform::MediaSourceError::out_of_range:
      return VfsError::open_failed;
    case platform::MediaSourceError::changed:
      return VfsError::source_changed;
    case platform::MediaSourceError::resource_exhausted:
      return VfsError::limit_exceeded;
    case platform::MediaSourceError::not_found:
    case platform::MediaSourceError::not_regular_file:
    case platform::MediaSourceError::open_failed:
    case platform::MediaSourceError::read_failed:
    case platform::MediaSourceError::unexpected_eof:
    case platform::MediaSourceError::unsupported:
      return VfsError::read_failed;
  }
  return VfsError::read_failed;
}

[[nodiscard]] std::string sanitized_label(const char* const label) {
  if (label == nullptr) {
    return {};
  }
  std::string result;
  result.reserve(128);
  for (const auto character : std::string_view{label}.substr(0, 128)) {
    const auto byte = static_cast<unsigned char>(character);
    result.push_back(byte >= 0x20U && byte <= 0x7eU ? character : '?');
  }
  return result;
}

}  // namespace

namespace detail {

BoundedDirectoryNameResult bounded_directory_name(
    const RawDirectoryName input) {
  BoundedDirectoryNameResult result;
  if (input.declared_size >
      static_cast<std::size_t>(
          UdfDirectoryLimits::hard_max_page_name_bytes)) {
    result.error = VfsError::limit_exceeded;
    return result;
  }
  if (!input.terminated) {
    result.error = VfsError::read_failed;
    return result;
  }
  if (input.data == nullptr && input.declared_size != 0) {
    result.error = VfsError::read_failed;
    return result;
  }
  if (input.declared_size == 0) {
    return result;
  }
  const std::string_view bytes{input.data, input.declared_size};
  if (bytes.find('\0') != std::string_view::npos) {
    result.error = VfsError::read_failed;
    return result;
  }
  result.name.assign(bytes);
  return result;
}

bool valid_directory_limits(const UdfArchiveLimits& limits) noexcept {
  const auto& directory = limits.directory;
  return limits.max_source_bytes != 0 &&
         limits.max_source_bytes <=
             UdfArchiveLimits::max_representable_source_bytes &&
         limits.max_blocks_per_read != 0 &&
         limits.max_blocks_per_read <=
             static_cast<std::uint32_t>(std::numeric_limits<int>::max()) &&
         directory.max_path_components != 0 &&
         directory.max_path_components <=
             UdfDirectoryLimits::hard_max_path_components &&
         directory.max_page_entries != 0 &&
         directory.max_page_entries <=
             UdfDirectoryLimits::hard_max_page_entries &&
         directory.max_page_name_bytes != 0 &&
         directory.max_page_name_bytes <=
             UdfDirectoryLimits::hard_max_page_name_bytes &&
         directory.max_page_result_bytes != 0 &&
         directory.max_page_result_bytes <=
             UdfDirectoryLimits::hard_max_page_result_bytes &&
         directory.max_page_work != 0 &&
         directory.max_page_work <=
             UdfDirectoryLimits::hard_max_page_work &&
         directory.max_page_count != 0 &&
         directory.max_page_count <=
             UdfDirectoryLimits::hard_max_page_count &&
         directory.max_cursor_work != 0 &&
         directory.max_cursor_work <=
             UdfDirectoryLimits::hard_max_cursor_work;
}

bool path_within_depth(const std::string_view normalized_path,
                       const UdfArchiveLimits& limits) noexcept {
  std::uint32_t depth = 0;
  bool in_component = false;
  for (const char character : normalized_path) {
    if (character == '/') {
      in_component = false;
      continue;
    }
    if (!in_component) {
      if (depth == limits.directory.max_path_components) {
        return false;
      }
      ++depth;
      in_component = true;
    }
  }
  return true;
}

struct DirectoryPageEngine::Impl {
  Impl(std::unique_ptr<DirectoryEntryProvider> input_provider,
       const UdfArchiveLimits input_limits)
      : provider{std::move(input_provider)}, limits{input_limits} {}

  [[nodiscard]] DirectoryProviderResult pull(std::uint64_t& page_work) {
    const auto& directory = limits.directory;
    while (true) {
      if (page_work >= directory.max_page_work ||
          cursor_work >= directory.max_cursor_work) {
        return {
            .error = VfsError::limit_exceeded, .end = false, .entry = {}};
      }
      ++page_work;
      ++cursor_work;
      auto result = provider->next();
      if (result.error != VfsError::none || result.end) {
        return result;
      }
      if (result.entry.name == "." || result.entry.name == "..") {
        continue;
      }
      if (result.entry.name.empty() ||
          result.entry.name.find('\0') != std::string::npos ||
          result.entry.name.find('/') != std::string::npos ||
          result.entry.name.find('\\') != std::string::npos) {
        return {.error = VfsError::read_failed, .end = false, .entry = {}};
      }
      return result;
    }
  }

  std::unique_ptr<DirectoryEntryProvider> provider;
  const UdfArchiveLimits limits;
  std::optional<DirectoryEntry> lookahead;
  std::uint64_t cursor_work{0};
  std::uint32_t page_count{0};
  bool terminal{false};
};

DirectoryPageEngine::DirectoryPageEngine(
    std::unique_ptr<DirectoryEntryProvider> provider,
    const UdfArchiveLimits limits)
    : implementation_{
          std::make_unique<Impl>(std::move(provider), limits)} {}

DirectoryPageEngine::~DirectoryPageEngine() = default;
DirectoryPageEngine::DirectoryPageEngine(DirectoryPageEngine&&) noexcept =
    default;
DirectoryPageEngine& DirectoryPageEngine::operator=(
    DirectoryPageEngine&&) noexcept = default;

DirectoryPageEngineResult DirectoryPageEngine::next_page() {
  DirectoryPageEngineResult result;
  auto fail = [&](const VfsError error) {
    result.error = error;
    result.entries.clear();
    result.has_more = false;
    implementation_->lookahead.reset();
    implementation_->terminal = true;
    return result;
  };

  if (implementation_ == nullptr || implementation_->provider == nullptr ||
      implementation_->terminal) {
    result.error = VfsError::invalid_cursor;
    return result;
  }
  if (!valid_directory_limits(implementation_->limits)) {
    return fail(VfsError::limit_exceeded);
  }
  const auto& limits = implementation_->limits.directory;
  if (implementation_->page_count >= limits.max_page_count) {
    return fail(VfsError::limit_exceeded);
  }
  ++implementation_->page_count;

  std::uint64_t page_work = 0;
  std::uint64_t page_name_bytes = 0;
  std::uint64_t page_result_bytes = 0;
  VfsError append_error = VfsError::none;
  auto append_or_defer = [&](DirectoryEntry entry) {
    if (!std::in_range<std::uint64_t>(entry.name.size())) {
      append_error = VfsError::limit_exceeded;
      return false;
    }
    const auto name_bytes = static_cast<std::uint64_t>(entry.name.size());
    if (name_bytes > std::numeric_limits<std::uint64_t>::max() -
                         UdfDirectoryLimits::logical_entry_metadata_bytes) {
      append_error = VfsError::limit_exceeded;
      return false;
    }
    const auto result_bytes =
        name_bytes + UdfDirectoryLimits::logical_entry_metadata_bytes;
    if (name_bytes > limits.max_page_name_bytes ||
        result_bytes > limits.max_page_result_bytes) {
      append_error = VfsError::limit_exceeded;
      return false;
    }
    if (result.entries.size() >=
            static_cast<std::size_t>(limits.max_page_entries) ||
        name_bytes > limits.max_page_name_bytes - page_name_bytes ||
        result_bytes > limits.max_page_result_bytes - page_result_bytes) {
      implementation_->lookahead = std::move(entry);
      result.has_more = true;
      return false;
    }
    page_name_bytes += name_bytes;
    page_result_bytes += result_bytes;
    result.entries.push_back(std::move(entry));
    return true;
  };

  if (implementation_->lookahead.has_value()) {
    auto entry = std::move(*implementation_->lookahead);
    implementation_->lookahead.reset();
    if (!append_or_defer(std::move(entry))) {
      return append_error == VfsError::none ? result : fail(append_error);
    }
  }

  while (true) {
    auto next = implementation_->pull(page_work);
    if (next.error != VfsError::none) {
      return fail(next.error);
    }
    if (next.end) {
      implementation_->terminal = true;
      return result;
    }
    if (!append_or_defer(std::move(next.entry))) {
      return append_error == VfsError::none ? result : fail(append_error);
    }
  }
}

}  // namespace detail

struct DirectoryCursor::Impl {
  std::shared_ptr<ArchiveState> owner;
  std::unique_ptr<detail::DirectoryPageEngine> engine;
};

DirectoryCursor::DirectoryCursor() = default;
DirectoryCursor::~DirectoryCursor() = default;
DirectoryCursor::DirectoryCursor(DirectoryCursor&&) noexcept = default;
DirectoryCursor& DirectoryCursor::operator=(DirectoryCursor&&) noexcept =
    default;

DirectoryCursor::DirectoryCursor(
    std::unique_ptr<Impl> implementation) noexcept
    : implementation_{std::move(implementation)} {}

bool DirectoryCursor::valid() const noexcept {
  return implementation_ != nullptr && implementation_->owner != nullptr &&
         implementation_->engine != nullptr;
}

DirectoryPage::DirectoryPage() = default;
DirectoryPage::~DirectoryPage() = default;
DirectoryPage::DirectoryPage(DirectoryPage&&) noexcept = default;
DirectoryPage& DirectoryPage::operator=(DirectoryPage&&) noexcept = default;

std::optional<std::string> normalize_path(const std::string_view path) {
  if (path.size() > 4'096 || path.find('\0') != std::string_view::npos) {
    return std::nullopt;
  }

  std::string normalized{"/"};
  std::size_t component_start = 0;
  while (component_start < path.size()) {
    while (component_start < path.size() &&
           (path[component_start] == '/' || path[component_start] == '\\')) {
      ++component_start;
    }
    if (component_start == path.size()) {
      break;
    }

    auto component_end = path.find_first_of("/\\", component_start);
    if (component_end == std::string_view::npos) {
      component_end = path.size();
    }
    const auto component =
        path.substr(component_start, component_end - component_start);
    if (component == "." || component == "..") {
      return std::nullopt;
    }
    if (normalized.size() > 1) {
      normalized.push_back('/');
    }
    normalized.append(component);
    component_start = component_end;
  }
  return normalized;
}

bool is_single_path_component(const std::string_view name) noexcept {
  return !name.empty() && name.size() <= 4'096 && name != "." &&
         name != ".." && name.find('\0') == std::string_view::npos &&
         name.find('/') == std::string_view::npos &&
         name.find('\\') == std::string_view::npos;
}

struct UdfFile::Impl {
  std::shared_ptr<ArchiveState> owner;
  UDFFILE* file{nullptr};
  std::uint64_t file_size{0};

  ~Impl() {
    if (file != nullptr) {
      const std::scoped_lock lock{owner->mutex};
      udfread_file_close(file);
    }
  }
};

UdfFile::UdfFile(std::unique_ptr<Impl> implementation) noexcept
    : implementation_{std::move(implementation)} {}

UdfFile::~UdfFile() = default;
UdfFile::UdfFile(UdfFile&&) noexcept = default;
UdfFile& UdfFile::operator=(UdfFile&&) noexcept = default;

std::uint64_t UdfFile::size() const noexcept {
  return implementation_ == nullptr ? 0 : implementation_->file_size;
}

std::int64_t UdfFile::tell() const noexcept {
  if (implementation_ == nullptr) {
    return -1;
  }
  const std::scoped_lock lock{implementation_->owner->mutex};
  return static_cast<std::int64_t>(
      udfread_file_tell(implementation_->file));
}

std::int64_t UdfFile::read(const std::span<std::byte> destination) {
  if (implementation_ == nullptr || destination.empty()) {
    return 0;
  }
  const auto bytes_to_read =
      std::min(destination.size(),
               static_cast<std::size_t>(
                   std::numeric_limits<std::int64_t>::max()));
  const std::scoped_lock lock{implementation_->owner->mutex};
  return static_cast<std::int64_t>(udfread_file_read(
      implementation_->file, destination.data(), bytes_to_read));
}

bool UdfFile::seek(const std::uint64_t offset) {
  if (implementation_ == nullptr ||
      offset > static_cast<std::uint64_t>(
                   std::numeric_limits<std::int64_t>::max())) {
    return false;
  }
  const std::scoped_lock lock{implementation_->owner->mutex};
  return udfread_file_seek(implementation_->file,
                           static_cast<std::int64_t>(offset), UDF_SEEK_SET) >= 0;
}

struct UdfArchive::Impl {
  std::shared_ptr<ArchiveState> state;
};

UdfArchive detail::DirectoryArchiveTestHarness::mount(
    std::shared_ptr<DirectoryProviderFactory> factory,
    const UdfArchiveLimits limits,
    std::shared_ptr<void> retained_lifetime) {
  UdfArchive result;
  if (factory == nullptr || !valid_directory_limits(limits)) {
    return result;
  }
  auto state = std::make_shared<ArchiveState>(SharedMediaSource{}, limits);
  state->synthetic_factory = std::move(factory);
  state->synthetic_lifetime = std::move(retained_lifetime);
  result.implementation_->state = std::move(state);
  return result;
}

UdfArchive::UdfArchive() : implementation_{std::make_unique<Impl>()} {}
UdfArchive::~UdfArchive() = default;
UdfArchive::UdfArchive(UdfArchive&&) noexcept = default;
UdfArchive& UdfArchive::operator=(UdfArchive&&) noexcept = default;

VfsError UdfArchive::open(SharedMediaSource source,
                          const UdfArchiveLimits limits) {
  if (implementation_ == nullptr) {
    implementation_ = std::make_unique<Impl>();
  }
  close();
  if (!detail::valid_directory_limits(limits)) {
    return VfsError::limit_exceeded;
  }

  auto input_result = detail::create_udf_media_input(source, limits);
  if (!input_result.valid()) {
    return input_result.error;
  }

  auto state = std::make_shared<ArchiveState>(source, limits);
  state->archive = udfread_init();
  if (state->archive == nullptr) {
    return VfsError::open_failed;
  }

  if (udfread_open_input(state->archive, input_result.input.get()) < 0) {
    return map_open_input_error(
        detail::udf_media_input_source_error(input_result.input.get()));
  }

  // libudfread owns a successful input and invokes its close callback from
  // udfread_close(). Failed opens never take ownership.
  (void)input_result.input.release();
  const auto stable = source->verify_unchanged();
  if (stable != platform::MediaSourceError::none) {
    return map_open_input_error(stable);
  }
  implementation_->state = std::move(state);
  return VfsError::none;
}

UdfArchive UdfArchive::share() const {
  UdfArchive result;
  if (implementation_ != nullptr) {
    result.implementation_->state = implementation_->state;
  }
  return result;
}

void UdfArchive::close() noexcept {
  if (implementation_ != nullptr) {
    implementation_->state.reset();
  }
}

bool UdfArchive::is_open() const noexcept {
  return implementation_ != nullptr && implementation_->state != nullptr &&
         (implementation_->state->archive != nullptr ||
          implementation_->state->synthetic_factory != nullptr);
}

std::string UdfArchive::volume_label() const {
  if (!is_open()) {
    return {};
  }
  const std::scoped_lock lock{implementation_->state->mutex};
  if (implementation_->state->archive == nullptr) {
    return {};
  }
  return sanitized_label(
      udfread_get_volume_id(implementation_->state->archive));
}

DirectoryPage UdfArchive::list_page(const std::string_view path) const {
  DirectoryPage result;
  if (!is_open()) {
    result.error = VfsError::not_open;
    return result;
  }

  const auto state = implementation_->state;
  const auto normalized = normalize_path(path);
  if (!normalized.has_value()) {
    result.error = VfsError::invalid_path;
    return result;
  }
  if (!detail::path_within_depth(*normalized, state->limits)) {
    result.error = VfsError::limit_exceeded;
    return result;
  }

  const std::scoped_lock lock{state->mutex};
  std::unique_ptr<detail::DirectoryEntryProvider> provider;
  if (state->synthetic_factory != nullptr) {
    auto opened = state->synthetic_factory->open(*normalized);
    if (opened.error != VfsError::none || opened.provider == nullptr) {
      result.error = opened.error == VfsError::none ? VfsError::read_failed
                                                     : opened.error;
      return result;
    }
    provider = std::move(opened.provider);
  } else {
    const auto initial_source_state = state->source->verify_unchanged();
    if (initial_source_state != platform::MediaSourceError::none) {
      result.error = map_open_input_error(initial_source_state);
      return result;
    }
    DirectoryHandle directory{
        udfread_opendir(state->archive, normalized->c_str())};
    if (!directory) {
      const auto final_source_state = state->source->verify_unchanged();
      result.error = final_source_state == platform::MediaSourceError::none
                         ? VfsError::not_found
                         : map_open_input_error(final_source_state);
      return result;
    }
    provider =
        std::make_unique<UdfDirectoryProvider>(state, directory.release());
  }

  auto engine = std::make_unique<detail::DirectoryPageEngine>(
      std::move(provider), state->limits);
  auto page = engine->next_page();
  if (state->source != nullptr) {
    const auto final_source_state = state->source->verify_unchanged();
    if (final_source_state != platform::MediaSourceError::none) {
      result.error = map_open_input_error(final_source_state);
      return result;
    }
  }
  if (page.error != VfsError::none) {
    result.error = page.error;
    return result;
  }

  result.entries = std::move(page.entries);
  if (page.has_more) {
    auto cursor = std::make_unique<DirectoryCursor::Impl>();
    cursor->owner = state;
    cursor->engine = std::move(engine);
    result.cursor = DirectoryCursor{std::move(cursor)};
  }
  return result;
}

DirectoryPage UdfArchive::continue_list(DirectoryCursor cursor) const {
  DirectoryPage result;
  if (!cursor.valid() || !is_open() ||
      cursor.implementation_->owner != implementation_->state) {
    result.error = VfsError::invalid_cursor;
    return result;
  }

  const auto state = cursor.implementation_->owner;
  auto engine = std::move(cursor.implementation_->engine);
  cursor.implementation_.reset();
  const std::scoped_lock lock{state->mutex};
  if (state->source != nullptr) {
    const auto initial_source_state = state->source->verify_unchanged();
    if (initial_source_state != platform::MediaSourceError::none) {
      result.error = map_open_input_error(initial_source_state);
      return result;
    }
  }
  auto page = engine->next_page();
  if (state->source != nullptr) {
    const auto final_source_state = state->source->verify_unchanged();
    if (final_source_state != platform::MediaSourceError::none) {
      result.error = map_open_input_error(final_source_state);
      return result;
    }
  }
  if (page.error != VfsError::none) {
    result.error = page.error;
    return result;
  }

  result.entries = std::move(page.entries);
  if (page.has_more) {
    auto next_cursor = std::make_unique<DirectoryCursor::Impl>();
    next_cursor->owner = state;
    next_cursor->engine = std::move(engine);
    result.cursor = DirectoryCursor{std::move(next_cursor)};
  }
  return result;
}

DirectoryListing UdfArchive::list(const std::string_view path) const {
  DirectoryListing result;
  auto page = list_page(path);
  while (true) {
    if (page.error != VfsError::none) {
      result.error = page.error;
      result.entries.clear();
      return result;
    }
    result.entries.insert(result.entries.end(),
                          std::make_move_iterator(page.entries.begin()),
                          std::make_move_iterator(page.entries.end()));
    if (page.complete()) {
      return result;
    }
    page = continue_list(std::move(page.cursor));
  }
}

std::unique_ptr<UdfFile> UdfArchive::open_file(
    const std::string_view path) const {
  if (!is_open()) {
    return nullptr;
  }
  if (implementation_->state->archive == nullptr) {
    return nullptr;
  }
  const auto normalized = normalize_path(path);
  if (!normalized.has_value() || *normalized == "/") {
    return nullptr;
  }

  const std::scoped_lock lock{implementation_->state->mutex};
  auto* const file =
      udfread_file_open(implementation_->state->archive, normalized->c_str());
  if (file == nullptr) {
    return nullptr;
  }
  const auto size = udfread_file_size(file);
  if (size < 0) {
    udfread_file_close(file);
    return nullptr;
  }

  auto file_implementation = std::make_unique<UdfFile::Impl>();
  file_implementation->owner = implementation_->state;
  file_implementation->file = file;
  file_implementation->file_size = static_cast<std::uint64_t>(size);
  return std::unique_ptr<UdfFile>{
      new UdfFile{std::move(file_implementation)}};
}

std::unique_ptr<UdfFile> UdfArchive::open_file_at(
    const std::string_view directory_path,
    const std::string_view entry_name) const {
  if (!is_open() || !is_single_path_component(entry_name)) {
    return nullptr;
  }
  if (implementation_->state->archive == nullptr) {
    return nullptr;
  }
  const auto directory_name = normalize_path(directory_path);
  if (!directory_name.has_value()) {
    return nullptr;
  }
  const std::scoped_lock lock{implementation_->state->mutex};
  DirectoryHandle directory{
      udfread_opendir(implementation_->state->archive,
                      directory_name->c_str())};
  if (!directory) {
    return nullptr;
  }
  const std::string name{entry_name};
  auto* const file = udfread_file_openat(directory.get(), name.c_str());
  if (file == nullptr) {
    return nullptr;
  }
  const auto size = udfread_file_size(file);
  if (size < 0) {
    udfread_file_close(file);
    return nullptr;
  }

  auto file_implementation = std::make_unique<UdfFile::Impl>();
  file_implementation->owner = implementation_->state;
  file_implementation->file = file;
  file_implementation->file_size = static_cast<std::uint64_t>(size);
  return std::unique_ptr<UdfFile>{
      new UdfFile{std::move(file_implementation)}};
}

}  // namespace ohl::vfs
