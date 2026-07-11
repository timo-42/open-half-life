#include "source_stability_internal.hpp"

#include "ohl/core/sha256.hpp"

#include <algorithm>
#include <array>
#include <cstddef>
#include <cstdint>
#include <span>

namespace ohl::media::detail {
namespace {

[[nodiscard]] SourceStabilityError map_boundary_error(
    const ohl::platform::MediaSourceError error) noexcept {
  if (error == ohl::platform::MediaSourceError::none) {
    return SourceStabilityError::none;
  }
  return error == ohl::platform::MediaSourceError::changed
             ? SourceStabilityError::source_changed
             : SourceStabilityError::read_failure;
}

}  // namespace

SourceStabilityError verify_complete_source_stability(
    const ValidatedMedia& media, const CancellationToken cancellation) {
  if (!media.valid()) {
    return SourceStabilityError::invalid_capability;
  }

  const auto& source = media.source();
  const auto& fingerprint = media.fingerprint();
  if (source == nullptr || source->size() != fingerprint.size_bytes) {
    return SourceStabilityError::invalid_capability;
  }

  auto result = map_boundary_error(source->verify_unchanged());
  if (result != SourceStabilityError::none) {
    return result;
  }
  if (cancellation.stop_requested()) {
    return SourceStabilityError::cancelled;
  }

  ohl::core::Sha256 sha256;
  std::array<std::byte, 64 * 1'024> buffer{};
  std::uint64_t offset = 0;
  while (offset < fingerprint.size_bytes) {
    const auto remaining = fingerprint.size_bytes - offset;
    const auto count = static_cast<std::size_t>(
        std::min<std::uint64_t>(remaining, buffer.size()));
    const auto destination = std::span{buffer}.first(count);
    const auto read_error = source->read_exact_at(offset, destination);
    if (read_error != ohl::platform::MediaSourceError::none) {
      result = map_boundary_error(source->verify_unchanged());
      if (result != SourceStabilityError::none) {
        return result;
      }
      return read_error == ohl::platform::MediaSourceError::unexpected_eof ||
                     read_error == ohl::platform::MediaSourceError::out_of_range
                 ? SourceStabilityError::source_changed
                 : SourceStabilityError::read_failure;
    }
    sha256.update(destination);
    offset += static_cast<std::uint64_t>(count);

    if (offset < fingerprint.size_bytes && cancellation.stop_requested()) {
      return SourceStabilityError::cancelled;
    }
  }

  result = map_boundary_error(source->verify_unchanged());
  if (result != SourceStabilityError::none) {
    return result;
  }
  return ohl::core::hex_encode(sha256.finish()) == fingerprint.sha256
             ? SourceStabilityError::none
             : SourceStabilityError::digest_mismatch;
}

}  // namespace ohl::media::detail
