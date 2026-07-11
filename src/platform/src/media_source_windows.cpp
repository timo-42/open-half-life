#include "ohl/platform/media_source.hpp"

#if defined(_WIN32)

#if !defined(NOMINMAX)
#define NOMINMAX
#endif
#include <windows.h>

#include <algorithm>
#include <cstddef>
#include <cstdint>
#include <limits>
#include <memory>
#include <new>
#include <utility>

namespace ohl::platform {
namespace {

struct NativeSnapshot {
  DWORD volume_serial{0};
  DWORD file_index_high{0};
  DWORD file_index_low{0};
  DWORD attributes{0};
  FILETIME last_write_time{};
  std::uint64_t size_bytes{0};
};

[[nodiscard]] MediaSourceError map_open_error(const DWORD error) noexcept {
  switch (error) {
    case ERROR_FILE_NOT_FOUND:
    case ERROR_PATH_NOT_FOUND:
      return MediaSourceError::not_found;
    case ERROR_TOO_MANY_OPEN_FILES:
    case ERROR_NOT_ENOUGH_MEMORY:
    case ERROR_OUTOFMEMORY:
      return MediaSourceError::resource_exhausted;
    default:
      return MediaSourceError::open_failed;
  }
}

[[nodiscard]] MediaSourceError map_read_error(const DWORD error) noexcept {
  switch (error) {
    case ERROR_HANDLE_EOF:
      return MediaSourceError::unexpected_eof;
    case ERROR_NOT_ENOUGH_MEMORY:
    case ERROR_OUTOFMEMORY:
    case ERROR_TOO_MANY_OPEN_FILES:
      return MediaSourceError::resource_exhausted;
    default:
      return MediaSourceError::read_failed;
  }
}

[[nodiscard]] bool query_snapshot(const HANDLE handle,
                                  NativeSnapshot& snapshot) noexcept {
  BY_HANDLE_FILE_INFORMATION information{};
  if (GetFileType(handle) != FILE_TYPE_DISK ||
      GetFileInformationByHandle(handle, &information) == 0) {
    return false;
  }
  snapshot.volume_serial = information.dwVolumeSerialNumber;
  snapshot.file_index_high = information.nFileIndexHigh;
  snapshot.file_index_low = information.nFileIndexLow;
  snapshot.attributes = information.dwFileAttributes;
  snapshot.last_write_time = information.ftLastWriteTime;
  snapshot.size_bytes =
      (static_cast<std::uint64_t>(information.nFileSizeHigh) << 32U) |
      static_cast<std::uint64_t>(information.nFileSizeLow);
  return true;
}

[[nodiscard]] bool is_regular_file(
    const NativeSnapshot& snapshot) noexcept {
  return (snapshot.attributes &
          (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_DEVICE |
           FILE_ATTRIBUTE_REPARSE_POINT)) == 0;
}

[[nodiscard]] bool is_reparse_point(
    const NativeSnapshot& snapshot) noexcept {
  return (snapshot.attributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0;
}

[[nodiscard]] bool same_file_time(const FILETIME& first,
                                  const FILETIME& second) noexcept {
  return first.dwHighDateTime == second.dwHighDateTime &&
         first.dwLowDateTime == second.dwLowDateTime;
}

class EventHandle final {
 public:
  explicit EventHandle(const HANDLE handle) noexcept : handle_(handle) {}
  ~EventHandle() {
    if (handle_ != nullptr) {
      (void)CloseHandle(handle_);
    }
  }

  EventHandle(const EventHandle&) = delete;
  EventHandle& operator=(const EventHandle&) = delete;

  [[nodiscard]] HANDLE get() const noexcept { return handle_; }

 private:
  HANDLE handle_{nullptr};
};

}  // namespace

struct MediaSource::Impl {
  Impl(const HANDLE source_handle,
       const NativeSnapshot& source_snapshot) noexcept
      : handle(source_handle), original_snapshot(source_snapshot) {}

  ~Impl() {
    if (handle != INVALID_HANDLE_VALUE) {
      (void)CloseHandle(handle);
    }
  }

  HANDLE handle{INVALID_HANDLE_VALUE};
  NativeSnapshot original_snapshot;
};

MediaSource::MediaSource(std::unique_ptr<Impl> implementation) noexcept
    : implementation_(std::move(implementation)) {}

MediaSource::~MediaSource() = default;

std::uint64_t MediaSource::size() const noexcept {
  return implementation_ == nullptr
             ? 0
             : implementation_->original_snapshot.size_bytes;
}

MediaSourceError MediaSource::read_exact_at(
    const std::uint64_t offset,
    const std::span<std::byte> destination) const noexcept {
  if (implementation_ == nullptr) {
    return MediaSourceError::read_failed;
  }

  const auto source_size = size();
  if (offset > source_size ||
      !std::in_range<std::uint64_t>(destination.size())) {
    return MediaSourceError::out_of_range;
  }
  const auto requested = static_cast<std::uint64_t>(destination.size());
  if (requested > source_size - offset) {
    return MediaSourceError::out_of_range;
  }
  if (destination.empty()) {
    return MediaSourceError::none;
  }

  EventHandle event{CreateEventW(nullptr, TRUE, FALSE, nullptr)};
  if (event.get() == nullptr) {
    return map_read_error(GetLastError());
  }

  std::size_t completed = 0;
  constexpr auto kMaximumNativeRead =
      static_cast<std::size_t>(std::numeric_limits<DWORD>::max());
  while (completed < destination.size()) {
    if (ResetEvent(event.get()) == 0) {
      return MediaSourceError::read_failed;
    }
    const auto chunk =
        std::min(destination.size() - completed, kMaximumNativeRead);
    const auto position =
        offset + static_cast<std::uint64_t>(completed);
    OVERLAPPED operation{};
    operation.Offset = static_cast<DWORD>(position & 0xffffffffULL);
    operation.OffsetHigh = static_cast<DWORD>(position >> 32U);
    operation.hEvent = event.get();

    if (ReadFile(implementation_->handle, destination.data() + completed,
                 static_cast<DWORD>(chunk), nullptr, &operation) == 0) {
      const auto error = GetLastError();
      if (error != ERROR_IO_PENDING) {
        return map_read_error(error);
      }
    }
    DWORD transferred = 0;
    if (GetOverlappedResult(implementation_->handle, &operation,
                            &transferred, TRUE) == 0) {
      return map_read_error(GetLastError());
    }
    if (transferred == 0) {
      return MediaSourceError::unexpected_eof;
    }
    if (transferred > chunk) {
      return MediaSourceError::read_failed;
    }
    completed += static_cast<std::size_t>(transferred);
  }
  return MediaSourceError::none;
}

MediaSourceError MediaSource::verify_unchanged() const noexcept {
  if (implementation_ == nullptr) {
    return MediaSourceError::read_failed;
  }

  NativeSnapshot current;
  if (!query_snapshot(implementation_->handle, current)) {
    return MediaSourceError::read_failed;
  }
  const auto& original = implementation_->original_snapshot;
  if (!is_regular_file(current) ||
      current.volume_serial != original.volume_serial ||
      current.file_index_high != original.file_index_high ||
      current.file_index_low != original.file_index_low ||
      current.size_bytes != original.size_bytes ||
      !same_file_time(current.last_write_time, original.last_write_time)) {
    return MediaSourceError::changed;
  }
  return MediaSourceError::none;
}

MediaSourceOpenResult open_media_source(
    const std::filesystem::path& path) noexcept {
  const auto& native_path = path.native();
  if (native_path.empty()) {
    return {.source = nullptr, .error = MediaSourceError::not_found};
  }
  if (native_path.find(L'\0') != std::filesystem::path::string_type::npos) {
    return {.source = nullptr, .error = MediaSourceError::open_failed};
  }

  const HANDLE handle = CreateFileW(
      native_path.c_str(), GENERIC_READ,
      FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, nullptr,
      OPEN_EXISTING,
      FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OVERLAPPED | FILE_FLAG_RANDOM_ACCESS |
          FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
      nullptr);
  if (handle == INVALID_HANDLE_VALUE) {
    return {.source = nullptr, .error = map_open_error(GetLastError())};
  }

  NativeSnapshot snapshot;
  if (!query_snapshot(handle, snapshot)) {
    (void)CloseHandle(handle);
    return {.source = nullptr, .error = MediaSourceError::open_failed};
  }
  if (is_reparse_point(snapshot)) {
    (void)CloseHandle(handle);
    return {.source = nullptr,
            .error = MediaSourceError::not_regular_file};
  }
  if (!is_regular_file(snapshot)) {
    (void)CloseHandle(handle);
    return {.source = nullptr,
            .error = MediaSourceError::not_regular_file};
  }

  auto implementation =
      std::unique_ptr<MediaSource::Impl>{
          new (std::nothrow) MediaSource::Impl{handle, snapshot}};
  if (implementation == nullptr) {
    (void)CloseHandle(handle);
    return {.source = nullptr,
            .error = MediaSourceError::resource_exhausted};
  }
  auto* const source =
      new (std::nothrow) MediaSource{std::move(implementation)};
  if (source == nullptr) {
    return {.source = nullptr,
            .error = MediaSourceError::resource_exhausted};
  }

  try {
    return {.source = std::shared_ptr<const MediaSource>{source},
            .error = MediaSourceError::none};
  } catch (...) {
    // shared_ptr deletes source when control-block allocation fails.
    return {.source = nullptr,
            .error = MediaSourceError::resource_exhausted};
  }
}

}  // namespace ohl::platform

#endif
