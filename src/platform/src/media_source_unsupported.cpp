#include "ohl/platform/media_source.hpp"

#include <cstdint>
#include <memory>
#include <utility>

namespace ohl::platform {

struct MediaSource::Impl {};

MediaSource::MediaSource(std::unique_ptr<Impl> implementation) noexcept
    : implementation_(std::move(implementation)) {}

MediaSource::~MediaSource() = default;

std::uint64_t MediaSource::size() const noexcept {
  return 0;
}

MediaSourceError MediaSource::read_exact_at(
    const std::uint64_t,
    const std::span<std::byte>) const noexcept {
  return MediaSourceError::unsupported;
}

MediaSourceError MediaSource::verify_unchanged() const noexcept {
  return MediaSourceError::unsupported;
}

MediaSourceOpenResult open_media_source(
    const std::filesystem::path&) noexcept {
  return {.source = nullptr, .error = MediaSourceError::unsupported};
}

}  // namespace ohl::platform
