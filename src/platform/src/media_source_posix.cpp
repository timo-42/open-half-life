#include "ohl/platform/media_source.hpp"

#if defined(__linux__) || defined(__APPLE__)

#include <fcntl.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>

#include <algorithm>
#include <cerrno>
#include <cstddef>
#include <cstdint>
#include <limits>
#include <memory>
#include <new>
#include <utility>

namespace ohl::platform {
namespace {

[[nodiscard]] MediaSourceError map_open_error(const int error) noexcept {
  switch (error) {
    case ENOENT:
    case ENOTDIR:
      return MediaSourceError::not_found;
    case ELOOP:
      return MediaSourceError::not_regular_file;
    case EMFILE:
    case ENFILE:
    case ENOMEM:
      return MediaSourceError::resource_exhausted;
    default:
      return MediaSourceError::open_failed;
  }
}

[[nodiscard]] bool same_modification_time(const struct stat& first,
                                          const struct stat& second) noexcept {
#if defined(__APPLE__)
  return first.st_mtimespec.tv_sec == second.st_mtimespec.tv_sec &&
         first.st_mtimespec.tv_nsec == second.st_mtimespec.tv_nsec;
#else
  return first.st_mtim.tv_sec == second.st_mtim.tv_sec &&
         first.st_mtim.tv_nsec == second.st_mtim.tv_nsec;
#endif
}

}  // namespace

struct MediaSource::Impl {
  Impl(const int source_descriptor, const struct stat& source_status) noexcept
      : descriptor(source_descriptor), original_status(source_status) {}

  ~Impl() {
    if (descriptor >= 0) {
      (void)::close(descriptor);
    }
  }

  int descriptor{-1};
  struct stat original_status {};
};

MediaSource::MediaSource(std::unique_ptr<Impl> implementation) noexcept
    : implementation_(std::move(implementation)) {}

MediaSource::~MediaSource() = default;

std::uint64_t MediaSource::size() const noexcept {
  return implementation_ == nullptr
             ? 0
             : static_cast<std::uint64_t>(
                   implementation_->original_status.st_size);
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

  std::size_t completed = 0;
  constexpr auto kMaximumNativeRead =
      static_cast<std::size_t>(std::numeric_limits<ssize_t>::max());
  while (completed < destination.size()) {
    const auto chunk =
        std::min(destination.size() - completed, kMaximumNativeRead);
    const auto position =
        offset + static_cast<std::uint64_t>(completed);
    const auto count = ::pread(
        implementation_->descriptor, destination.data() + completed, chunk,
        static_cast<off_t>(position));
    if (count > 0) {
      completed += static_cast<std::size_t>(count);
      continue;
    }
    if (count == 0) {
      return MediaSourceError::unexpected_eof;
    }
    if (errno != EINTR) {
      return MediaSourceError::read_failed;
    }
  }
  return MediaSourceError::none;
}

MediaSourceError MediaSource::verify_unchanged() const noexcept {
  if (implementation_ == nullptr) {
    return MediaSourceError::read_failed;
  }

  struct stat current_status {};
  if (::fstat(implementation_->descriptor, &current_status) != 0) {
    return MediaSourceError::read_failed;
  }
  const auto& original = implementation_->original_status;
  if (!S_ISREG(current_status.st_mode) || current_status.st_size < 0 ||
      current_status.st_dev != original.st_dev ||
      current_status.st_ino != original.st_ino ||
      current_status.st_size != original.st_size ||
      !same_modification_time(current_status, original)) {
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
  if (native_path.find('\0') != std::filesystem::path::string_type::npos) {
    return {.source = nullptr, .error = MediaSourceError::open_failed};
  }

  const int descriptor =
      ::open(native_path.c_str(),
             O_RDONLY | O_CLOEXEC | O_NONBLOCK | O_NOCTTY | O_NOFOLLOW);
  if (descriptor < 0) {
    return {.source = nullptr, .error = map_open_error(errno)};
  }

  struct stat status {};
  if (::fstat(descriptor, &status) != 0) {
    (void)::close(descriptor);
    return {.source = nullptr, .error = MediaSourceError::open_failed};
  }
  if (!S_ISREG(status.st_mode)) {
    (void)::close(descriptor);
    return {.source = nullptr,
            .error = MediaSourceError::not_regular_file};
  }
  if (status.st_size < 0) {
    (void)::close(descriptor);
    return {.source = nullptr, .error = MediaSourceError::open_failed};
  }

  auto implementation =
      std::unique_ptr<MediaSource::Impl>{
          new (std::nothrow) MediaSource::Impl{descriptor, status}};
  if (implementation == nullptr) {
    (void)::close(descriptor);
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
