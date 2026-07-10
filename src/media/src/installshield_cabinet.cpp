#include "ohl/media/installshield_cabinet.hpp"

#include "ohl/media/payload_path.hpp"
#include "ohl/vfs/udf_archive.hpp"

#include <dirent.h>
#include <libunshield.h>

#include <cerrno>
#include <climits>
#include <cstdio>
#include <cstring>
#include <limits>
#include <memory>
#include <span>
#include <string>
#include <utility>
#include <vector>

namespace ohl::media {
namespace {

struct CabinetContext {
  ohl::vfs::UdfArchive source;
};

struct DirectoryCursor {
  std::vector<std::string> names;
  std::size_t next{0};
  dirent current{};
};

void* open_file(const char* filename, const char* modes,
                void* user_data) noexcept {
  if (filename == nullptr || modes == nullptr ||
      std::strchr(modes, 'r') == nullptr) {
    return nullptr;
  }
  const auto* const context = static_cast<CabinetContext*>(user_data);
  if (context == nullptr || !context->source.is_open()) {
    return nullptr;
  }
  try {
    return context->source.open_file(filename).release();
  } catch (...) {
    return nullptr;
  }
}

int seek_file(void* file, const long int offset, const int origin,
              void*) {
  auto* const input = static_cast<ohl::vfs::UdfFile*>(file);
  if (input == nullptr) {
    return -1;
  }

  std::uint64_t base = 0;
  if (origin == SEEK_CUR) {
    const auto position = input->tell();
    if (position < 0) {
      return -1;
    }
    base = static_cast<std::uint64_t>(position);
  } else if (origin == SEEK_END) {
    base = input->size();
  } else if (origin != SEEK_SET) {
    return -1;
  }

  std::uint64_t target = base;
  if (offset < 0) {
    const auto magnitude = static_cast<std::uint64_t>(-(offset + 1)) + 1U;
    if (magnitude > base) {
      return -1;
    }
    target -= magnitude;
  } else {
    const auto positive_offset = static_cast<std::uint64_t>(offset);
    if (positive_offset > std::numeric_limits<std::uint64_t>::max() - base) {
      return -1;
    }
    target += positive_offset;
  }
  return input->seek(target) ? 0 : -1;
}

long int tell_file(void* file, void*) {
  const auto* const input = static_cast<ohl::vfs::UdfFile*>(file);
  if (input == nullptr) {
    return -1;
  }
  const auto position = input->tell();
  if (position < 0 || position > LONG_MAX) {
    return -1;
  }
  return static_cast<long int>(position);
}

std::size_t read_file(void* destination, const std::size_t element_size,
                      const std::size_t element_count, void* file, void*) {
  auto* const input = static_cast<ohl::vfs::UdfFile*>(file);
  if (input == nullptr || destination == nullptr || element_size == 0 ||
      element_count == 0 ||
      element_count > std::numeric_limits<std::size_t>::max() / element_size) {
    return 0;
  }

  const auto byte_count = element_size * element_count;
  const auto bytes_read = input->read(
      std::span<std::byte>{static_cast<std::byte*>(destination), byte_count});
  return bytes_read < 0 ? 0 : static_cast<std::size_t>(bytes_read) / element_size;
}

std::size_t reject_write(const void*, std::size_t, std::size_t, void*, void*) {
  return 0;
}

int close_file(void* file, void*) {
  delete static_cast<ohl::vfs::UdfFile*>(file);
  return 0;
}

void* open_directory(const char* name, void* user_data) noexcept {
  const auto* const context = static_cast<CabinetContext*>(user_data);
  if (context == nullptr || !context->source.is_open() || name == nullptr) {
    return nullptr;
  }
  try {
    auto listing = context->source.list(name);
    if (listing.error != ohl::vfs::VfsError::none) {
      return nullptr;
    }

    auto cursor = std::make_unique<DirectoryCursor>();
    cursor->names.reserve(listing.entries.size());
    for (auto& entry : listing.entries) {
      cursor->names.push_back(std::move(entry.name));
    }
    return cursor.release();
  } catch (...) {
    return nullptr;
  }
}

int close_directory(void* directory, void*) {
  delete static_cast<DirectoryCursor*>(directory);
  return 0;
}

dirent* read_directory(void* directory, void*) {
  auto* const cursor = static_cast<DirectoryCursor*>(directory);
  if (cursor == nullptr) {
    return nullptr;
  }
  while (cursor->next < cursor->names.size()) {
    const auto& name = cursor->names[cursor->next++];
    if (name.size() >= sizeof(cursor->current.d_name)) {
      continue;
    }
    std::memset(&cursor->current, 0, sizeof(cursor->current));
    std::memcpy(cursor->current.d_name, name.data(), name.size());
    return &cursor->current;
  }
  return nullptr;
}

const UnshieldIoCallbacks kReadOnlyCallbacks{
    .fopen = open_file,
    .fseek = seek_file,
    .ftell = tell_file,
    .fread = read_file,
    .fwrite = reject_write,
    .fclose = close_file,
    .opendir = open_directory,
    .closedir = close_directory,
    .readdir = read_directory,
};

[[nodiscard]] constexpr bool is_valid_directory_index(
    const int index, const int count) noexcept {
  return index == -1 || (index >= 0 && index < count);
}

static_assert(is_valid_directory_index(-1, 0));
static_assert(!is_valid_directory_index(-2, 1));
static_assert(is_valid_directory_index(0, 1));
static_assert(!is_valid_directory_index(1, 1));

[[nodiscard]] std::string safe_relative_path(const char* const directory,
                                             const char* const name) {
  if (name == nullptr) {
    return {};
  }
  const std::string_view file_name{name};
  if (file_name.find_first_of("/\\") != std::string_view::npos) {
    return {};
  }

  std::string combined;
  if (directory != nullptr && *directory != '\0') {
    combined = directory;
    combined.push_back('/');
  }
  combined.append(file_name);
  return validate_payload_path(combined).relative_path;
}

}  // namespace

struct InstallShieldCabinet::Impl {
  CabinetContext context;
  Unshield* cabinet{nullptr};

  ~Impl() {
    if (cabinet != nullptr) {
      unshield_close(cabinet);
    }
  }
};

InstallShieldCabinet::InstallShieldCabinet()
    : implementation_{std::make_unique<Impl>()} {}
InstallShieldCabinet::~InstallShieldCabinet() = default;
InstallShieldCabinet::InstallShieldCabinet(InstallShieldCabinet&&) noexcept =
    default;
InstallShieldCabinet& InstallShieldCabinet::operator=(
    InstallShieldCabinet&&) noexcept = default;

CabinetError InstallShieldCabinet::open(const ohl::vfs::UdfArchive& source,
                                        const std::string_view cabinet_path) {
  close();
  if (!source.is_open()) {
    return CabinetError::source_not_open;
  }
  const auto normalized = ohl::vfs::normalize_path(cabinet_path);
  if (!normalized.has_value() || *normalized == "/") {
    return CabinetError::invalid_path;
  }
  if (implementation_ == nullptr) {
    implementation_ = std::make_unique<Impl>();
  }
  implementation_->context.source = source.share();
  unshield_set_log_level(UNSHIELD_LOG_LEVEL_ERROR);
  implementation_->cabinet = unshield_open2(normalized->c_str(),
                                             &kReadOnlyCallbacks,
                                             &implementation_->context);
  return implementation_->cabinet == nullptr
             ? CabinetError::unsupported_or_corrupt
             : CabinetError::none;
}

void InstallShieldCabinet::close() noexcept {
  if (implementation_ == nullptr) {
    return;
  }
  if (implementation_->cabinet != nullptr) {
    unshield_close(implementation_->cabinet);
    implementation_->cabinet = nullptr;
  }
  implementation_->context.source.close();
}

bool InstallShieldCabinet::is_open() const noexcept {
  return implementation_ != nullptr && implementation_->cabinet != nullptr;
}

CabinetListing InstallShieldCabinet::entries(
    const std::size_t maximum_entries) const {
  CabinetListing result;
  if (!is_open()) {
    result.error = CabinetError::source_not_open;
    return result;
  }
  const auto count = unshield_file_count(implementation_->cabinet);
  if (count < 0) {
    result.error = CabinetError::unsupported_or_corrupt;
    return result;
  }
  if (static_cast<std::size_t>(count) > maximum_entries) {
    result.error = CabinetError::too_many_entries;
    return result;
  }
  result.total_entries = static_cast<std::size_t>(count);
  result.entries.reserve(static_cast<std::size_t>(count));
  const auto directory_count =
      unshield_directory_count(implementation_->cabinet);
  const auto reject_entry = [&result](const CabinetError error) {
    if (result.error == CabinetError::none) {
      result.error = error;
    }
    ++result.rejected_entries;
  };
  for (int index = 0; index < count; ++index) {
    if (!unshield_file_is_valid(implementation_->cabinet, index)) {
      reject_entry(CabinetError::unsupported_or_corrupt);
      continue;
    }
    const auto* const name =
        unshield_file_name(implementation_->cabinet, index);
    const auto directory_index =
        unshield_file_directory(implementation_->cabinet, index);
    const char* directory = nullptr;
    if (!is_valid_directory_index(directory_index, directory_count)) {
      reject_entry(CabinetError::unsupported_or_corrupt);
      continue;
    }
    if (directory_index != -1) {
      directory = unshield_directory_name(implementation_->cabinet,
                                           directory_index);
      if (directory == nullptr) {
        reject_entry(CabinetError::unsupported_or_corrupt);
        continue;
      }
    }
    auto path = safe_relative_path(directory, name);
    if (path.empty()) {
      reject_entry(CabinetError::invalid_entry_path);
      continue;
    }
    result.entries.push_back(CabinetEntry{
        .index = index,
        .safe_relative_path = std::move(path),
        .size_bytes = static_cast<std::uint64_t>(
            unshield_file_size(implementation_->cabinet, index)),
    });
  }
  return result;
}

}  // namespace ohl::media
