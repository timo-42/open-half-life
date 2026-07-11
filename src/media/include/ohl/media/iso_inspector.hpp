#pragma once

#include "ohl/platform/media_source.hpp"

#include <cstdint>
#include <memory>
#include <optional>
#include <string>
#include <string_view>

namespace ohl::media {

using SharedMediaSource =
    std::shared_ptr<const ohl::platform::MediaSource>;

enum class MediaError {
  none,
  too_small,
  source_too_large,
  source_changed,
  unsupported_filesystem,
  invalid_structure,
  io_error,
};

[[nodiscard]] constexpr std::string_view to_string(
    const MediaError error) noexcept {
  switch (error) {
    case MediaError::none:
      return "none";
    case MediaError::too_small:
      return "file is too small to contain a supported UDF image";
    case MediaError::source_too_large:
      return "media exceeds the configured validation limit";
    case MediaError::source_changed:
      return "media changed during validation";
    case MediaError::unsupported_filesystem:
      return "media does not contain a supported ECMA-167 NSR02 structure";
    case MediaError::invalid_structure:
      return "UDF descriptor structure is invalid or truncated";
    case MediaError::io_error:
      return "media could not be read";
  }

  return "unknown media error";
}

struct SourceFingerprint {
  std::uint64_t size_bytes{0};
  std::string sha256;

  [[nodiscard]] bool valid() const noexcept {
    return size_bytes != 0 && sha256.size() == 64;
  }

  [[nodiscard]] bool operator==(const SourceFingerprint&) const = default;
};

struct IsoInspection {
  MediaError error{MediaError::none};
  std::uint64_t size_bytes{0};
  std::string source_sha256;
  std::string filesystem;
  std::string volume_label;

  [[nodiscard]] bool valid() const noexcept {
    return error == MediaError::none;
  }
};

// A validation budget bounds all bytes hashed from an untrusted source. The
// default comfortably covers CD and dual-layer DVD media without permitting
// an accidental unbounded scan.
struct IsoValidationLimits {
  std::uint64_t maximum_source_bytes{16ULL * 1'024ULL * 1'024ULL * 1'024ULL};
};

struct IsoValidationResult;

// Proof that one pinned MediaSource passed structural validation and full
// content fingerprinting without changing at the validation boundaries.
// Construction is restricted to validate_iso(). Moving transfers the proof;
// a moved-from value is invalid and is rejected by downstream APIs.
class ValidatedMedia final {
 public:
  ValidatedMedia(const ValidatedMedia&) = delete;
  ValidatedMedia& operator=(const ValidatedMedia&) = delete;
  ValidatedMedia(ValidatedMedia&&) noexcept = default;
  ValidatedMedia& operator=(ValidatedMedia&&) noexcept = default;

  [[nodiscard]] bool valid() const noexcept {
    return source_ != nullptr && inspection_.valid() && fingerprint_.valid() &&
           inspection_.size_bytes == fingerprint_.size_bytes &&
           inspection_.source_sha256 == fingerprint_.sha256;
  }

  [[nodiscard]] const SharedMediaSource& source() const noexcept {
    return source_;
  }

  [[nodiscard]] const SourceFingerprint& fingerprint() const noexcept {
    return fingerprint_;
  }

  [[nodiscard]] const IsoInspection& inspection() const noexcept {
    return inspection_;
  }

 private:
  ValidatedMedia(SharedMediaSource source, IsoInspection inspection,
                 SourceFingerprint fingerprint) noexcept;

  SharedMediaSource source_;
  IsoInspection inspection_;
  SourceFingerprint fingerprint_;

  friend IsoValidationResult validate_iso(
      SharedMediaSource source, IsoValidationLimits limits);
};

struct IsoValidationResult {
  MediaError error{MediaError::none};
  std::optional<ValidatedMedia> media;

  [[nodiscard]] bool valid() const noexcept;
};

inline bool IsoValidationResult::valid() const noexcept {
  return error == MediaError::none && media.has_value() && media->valid();
}

// Validates and fingerprints exactly one previously acquired native file
// object. All reads are positional and bounded by both the acquisition size
// and limits.maximum_source_bytes. The original pathname is never consulted.
[[nodiscard]] IsoValidationResult validate_iso(
    SharedMediaSource source, IsoValidationLimits limits = {});

}  // namespace ohl::media
