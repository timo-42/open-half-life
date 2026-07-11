#include "ohl/vfs/udf_archive.hpp"

#include "ohl/platform/media_source.hpp"
#include "udf_media_input.hpp"

#if defined(_MSC_VER)
#include <BaseTsd.h>
typedef SSIZE_T ssize_t;
#endif
#include <udfread/udfread.h>

#include <algorithm>
#include <limits>
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

struct ArchiveState {
  udfread* archive{nullptr};

  ~ArchiveState() {
    if (archive != nullptr) {
      udfread_close(archive);
    }
  }
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
  return implementation_ == nullptr
             ? -1
             : static_cast<std::int64_t>(
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
  return static_cast<std::int64_t>(udfread_file_read(
      implementation_->file, destination.data(), bytes_to_read));
}

bool UdfFile::seek(const std::uint64_t offset) {
  if (implementation_ == nullptr ||
      offset > static_cast<std::uint64_t>(
                   std::numeric_limits<std::int64_t>::max())) {
    return false;
  }
  return udfread_file_seek(implementation_->file,
                           static_cast<std::int64_t>(offset), UDF_SEEK_SET) >= 0;
}

struct UdfArchive::Impl {
  std::shared_ptr<ArchiveState> state;
};

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

  auto input_result = detail::create_udf_media_input(source, limits);
  if (!input_result.valid()) {
    return input_result.error;
  }

  auto state = std::make_shared<ArchiveState>();
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
         implementation_->state->archive != nullptr;
}

std::string UdfArchive::volume_label() const {
  if (!is_open()) {
    return {};
  }
  return sanitized_label(
      udfread_get_volume_id(implementation_->state->archive));
}

DirectoryListing UdfArchive::list(const std::string_view path) const {
  DirectoryListing result;
  if (!is_open()) {
    result.error = VfsError::not_open;
    return result;
  }

  const auto normalized = normalize_path(path);
  if (!normalized.has_value()) {
    result.error = VfsError::invalid_path;
    return result;
  }

  DirectoryHandle directory{
      udfread_opendir(implementation_->state->archive, normalized->c_str())};
  if (!directory) {
    result.error = VfsError::not_found;
    return result;
  }

  udfread_dirent entry{};
  while (udfread_readdir(directory.get(), &entry) != nullptr) {
    if (entry.d_name == nullptr || std::string_view{entry.d_name} == "." ||
        std::string_view{entry.d_name} == "..") {
      continue;
    }

    DirectoryEntry item{
        .name = entry.d_name,
        .type = entry_type(entry.d_type),
        .size_bytes = 0,
    };
    if (entry.d_type == UDF_DT_REG) {
      auto* const file = udfread_file_openat(directory.get(), entry.d_name);
      if (file == nullptr) {
        result.error = VfsError::read_failed;
        return result;
      }
      const auto size = udfread_file_size(file);
      udfread_file_close(file);
      if (size < 0) {
        result.error = VfsError::read_failed;
        return result;
      }
      item.size_bytes = static_cast<std::uint64_t>(size);
    }
    result.entries.push_back(std::move(item));
  }
  return result;
}

std::unique_ptr<UdfFile> UdfArchive::open_file(
    const std::string_view path) const {
  if (!is_open()) {
    return nullptr;
  }
  const auto normalized = normalize_path(path);
  if (!normalized.has_value() || *normalized == "/") {
    return nullptr;
  }

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
  const auto directory_name = normalize_path(directory_path);
  if (!directory_name.has_value()) {
    return nullptr;
  }
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
