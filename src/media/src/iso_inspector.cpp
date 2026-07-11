#include "ohl/media/iso_inspector.hpp"

#include "ohl/core/sha256.hpp"

#include <algorithm>
#include <array>
#include <cstddef>
#include <cstdint>
#include <limits>
#include <span>
#include <string_view>
#include <utility>

namespace ohl::media {
namespace {

constexpr std::uint64_t kSectorSize = 2'048;
constexpr std::uint64_t kAnchorSector = 256;
constexpr std::uint64_t kFirstRecognitionSector = 16;
constexpr std::uint64_t kRecognitionScanLimit = 64;
constexpr std::uint64_t kDescriptorScanLimit = 256;
constexpr std::size_t kDescriptorTagSize = 16;
using Sector = std::array<std::byte, kSectorSize>;

[[nodiscard]] std::uint8_t byte_at(const std::span<const std::byte> bytes,
                                   const std::size_t offset) noexcept {
  return std::to_integer<std::uint8_t>(bytes[offset]);
}

[[nodiscard]] std::uint16_t read_little_u16(
    const std::span<const std::byte> bytes, const std::size_t offset) noexcept {
  return static_cast<std::uint16_t>(byte_at(bytes, offset)) |
         static_cast<std::uint16_t>(
             static_cast<std::uint16_t>(byte_at(bytes, offset + 1)) << 8U);
}

[[nodiscard]] std::uint32_t read_little_u32(
    const std::span<const std::byte> bytes, const std::size_t offset) noexcept {
  return static_cast<std::uint32_t>(byte_at(bytes, offset)) |
         (static_cast<std::uint32_t>(byte_at(bytes, offset + 1)) << 8U) |
         (static_cast<std::uint32_t>(byte_at(bytes, offset + 2)) << 16U) |
         (static_cast<std::uint32_t>(byte_at(bytes, offset + 3)) << 24U);
}

[[nodiscard]] std::uint16_t crc_itu_t(
    const std::span<const std::byte> bytes) noexcept {
  std::uint16_t crc = 0;
  for (const auto value : bytes) {
    crc ^= static_cast<std::uint16_t>(
        static_cast<std::uint16_t>(std::to_integer<std::uint8_t>(value))
        << 8U);
    for (int bit = 0; bit < 8; ++bit) {
      const bool high_bit_set = (crc & 0x8000U) != 0;
      crc = static_cast<std::uint16_t>(crc << 1U);
      if (high_bit_set) {
        crc ^= 0x1021U;
      }
    }
  }
  return crc;
}

[[nodiscard]] bool read_sector(const SharedMediaSource& source,
                               const std::uint64_t sector_number,
                               Sector& destination) {
  if (source == nullptr ||
      sector_number >
          std::numeric_limits<std::uint64_t>::max() / kSectorSize) {
    return false;
  }
  return source->read_exact_at(sector_number * kSectorSize, destination) ==
         ohl::platform::MediaSourceError::none;
}

[[nodiscard]] bool fingerprint_source(const SharedMediaSource& source,
                                      const std::uint64_t source_size,
                                      const std::uint64_t maximum_bytes,
                                      std::string& digest) {
  if (source == nullptr || source_size > maximum_bytes) {
    return false;
  }
  ohl::core::Sha256 sha256;
  std::array<std::byte, 64 * 1'024> buffer{};
  std::uint64_t offset = 0;
  while (offset < source_size) {
    const auto remaining = source_size - offset;
    const auto count = static_cast<std::size_t>(
        std::min<std::uint64_t>(remaining, buffer.size()));
    auto destination = std::span{buffer}.first(count);
    if (source->read_exact_at(offset, destination) !=
        ohl::platform::MediaSourceError::none) {
      return false;
    }
    sha256.update(destination);
    offset += static_cast<std::uint64_t>(count);
  }
  digest = ohl::core::hex_encode(sha256.finish());
  return true;
}

[[nodiscard]] bool identifier_is(const Sector& sector,
                                 const std::string_view identifier) noexcept {
  if (identifier.size() != 5 || byte_at(sector, 0) != 0 ||
      byte_at(sector, 6) != 1) {
    return false;
  }

  for (std::size_t index = 0; index < identifier.size(); ++index) {
    if (byte_at(sector, index + 1) !=
        static_cast<std::uint8_t>(identifier[index])) {
      return false;
    }
  }
  return std::all_of(sector.begin() + 7, sector.end(),
                     [](const std::byte value) {
                       return value == std::byte{0};
                     });
}

[[nodiscard]] bool has_udf_102_recognition_sequence(
    const SharedMediaSource& source, bool& read_failed) {
  bool found_beginning = false;
  bool found_nsr02 = false;
  Sector sector{};

  for (std::uint64_t sector_number = kFirstRecognitionSector;
       sector_number < kRecognitionScanLimit; ++sector_number) {
    if (!read_sector(source, sector_number, sector)) {
      read_failed = true;
      return false;
    }
    if (!found_beginning) {
      found_beginning = identifier_is(sector, "BEA01");
      continue;
    }
    if (!found_nsr02) {
      if (!identifier_is(sector, "NSR02")) {
        return false;
      }
      found_nsr02 = true;
      continue;
    }
    return identifier_is(sector, "TEA01");
  }

  return false;
}

[[nodiscard]] bool valid_descriptor_tag(
    const Sector& sector, const std::uint16_t expected_identifier,
    const std::uint32_t expected_location) noexcept {
  const std::span<const std::byte> bytes{sector};
  if (read_little_u16(bytes, 0) != expected_identifier ||
      read_little_u16(bytes, 2) != 2 || byte_at(bytes, 5) != 0 ||
      read_little_u32(bytes, 12) != expected_location) {
    return false;
  }

  std::uint8_t checksum = 0;
  for (std::size_t index = 0; index < kDescriptorTagSize; ++index) {
    if (index != 4) {
      checksum = static_cast<std::uint8_t>(checksum + byte_at(bytes, index));
    }
  }
  if (checksum != byte_at(bytes, 4)) {
    return false;
  }

  std::uint64_t expected_crc_length = 496;
  if (expected_identifier == 6) {
    expected_crc_length =
        424U + static_cast<std::uint64_t>(read_little_u32(bytes, 264));
  } else if (expected_identifier == 7) {
    expected_crc_length =
        8U + (8U * static_cast<std::uint64_t>(read_little_u32(bytes, 20)));
  }
  const auto crc_length = read_little_u16(bytes, 10);
  if (expected_crc_length > sector.size() - kDescriptorTagSize ||
      crc_length != expected_crc_length) {
    return false;
  }
  const auto recorded_crc = read_little_u16(bytes, 8);
  const auto payload = bytes.subspan(kDescriptorTagSize, crc_length);
  return crc_itu_t(payload) == recorded_crc;
}

[[nodiscard]] std::string decode_dstring(
    const std::span<const std::byte> field) {
  if (field.size() < 2) {
    return {};
  }

  const auto encoded_length = byte_at(field, field.size() - 1);
  if (encoded_length < 1 || encoded_length > field.size() - 1) {
    return {};
  }

  const auto compression = byte_at(field, 0);
  std::string decoded;
  if (compression == 8) {
    for (std::size_t index = 1; index < encoded_length; ++index) {
      const auto character = byte_at(field, index);
      decoded.push_back(character >= 0x20U && character <= 0x7eU
                            ? static_cast<char>(character)
                            : '?');
    }
  } else if (compression == 16 && (encoded_length % 2U) == 1U) {
    for (std::size_t index = 1; index + 1 < encoded_length; index += 2) {
      const auto high = byte_at(field, index);
      const auto low = byte_at(field, index + 1);
      decoded.push_back(high == 0 && low >= 0x20U && low <= 0x7eU
                            ? static_cast<char>(low)
                            : '?');
    }
  }
  return decoded;
}

[[nodiscard]] bool extent_is_in_bounds(const std::uint32_t byte_length,
                                       const std::uint32_t start_sector,
                                       const std::uint64_t sector_count) {
  if (byte_length < 512) {
    return false;
  }
  const auto extent_sectors =
      (static_cast<std::uint64_t>(byte_length) + kSectorSize - 1U) /
      kSectorSize;
  return start_sector < sector_count && extent_sectors <= sector_count &&
         static_cast<std::uint64_t>(start_sector) <=
             sector_count - extent_sectors;
}

[[nodiscard]] bool inspect_volume_descriptor_sequence(
    const SharedMediaSource& source, const std::uint32_t byte_length,
    const std::uint32_t start_sector, std::string& volume_label,
    bool& read_failed) {
  const auto extent_sectors =
      (static_cast<std::uint64_t>(byte_length) + kSectorSize - 1U) /
      kSectorSize;
  const auto sectors_to_scan =
      std::min(extent_sectors, kDescriptorScanLimit);
  bool found_primary = false;
  bool found_partition = false;
  bool found_logical = false;
  bool found_terminator = false;
  Sector sector{};

  for (std::uint64_t index = 0; index < sectors_to_scan; ++index) {
    const auto location = static_cast<std::uint64_t>(start_sector) + index;
    if (!read_sector(source, location, sector)) {
      read_failed = true;
      return false;
    }
    const std::span<const std::byte> bytes{sector};
    const auto identifier = read_little_u16(bytes, 0);
    if (identifier == 0) {
      break;
    }
    if (location > std::numeric_limits<std::uint32_t>::max() ||
        !valid_descriptor_tag(sector, identifier,
                              static_cast<std::uint32_t>(location))) {
      return false;
    }

    switch (identifier) {
      case 1:
        found_primary = true;
        volume_label = decode_dstring(bytes.subspan(24, 32));
        break;
      case 5:
        found_partition = true;
        break;
      case 6:
        found_logical = read_little_u32(bytes, 212) == kSectorSize;
        break;
      case 8:
        found_terminator = true;
        break;
      default:
        break;
    }

    if (found_terminator) {
      break;
    }
  }

  return found_primary && found_partition && found_logical && found_terminator;
}

}  // namespace

ValidatedMedia::ValidatedMedia(SharedMediaSource source,
                               IsoInspection inspection,
                               SourceFingerprint fingerprint) noexcept
    : source_(std::move(source)),
      inspection_(std::move(inspection)),
      fingerprint_(std::move(fingerprint)) {}

namespace {

[[nodiscard]] MediaError verify_source(
    const SharedMediaSource& source) noexcept {
  if (source == nullptr) {
    return MediaError::io_error;
  }
  const auto error = source->verify_unchanged();
  if (error == ohl::platform::MediaSourceError::none) {
    return MediaError::none;
  }
  return error == ohl::platform::MediaSourceError::changed
             ? MediaError::source_changed
             : MediaError::io_error;
}

[[nodiscard]] MediaError phase_error(
    const SharedMediaSource& source, const MediaError fallback) noexcept {
  const auto verification = verify_source(source);
  return verification == MediaError::none ? fallback : verification;
}

}  // namespace

IsoValidationResult validate_iso(SharedMediaSource source,
                                 const IsoValidationLimits limits) {
  IsoValidationResult result;
  result.error = verify_source(source);
  if (result.error != MediaError::none) {
    return result;
  }

  IsoInspection inspection;
  inspection.size_bytes = source->size();
  if (inspection.size_bytes > limits.maximum_source_bytes) {
    result.error = MediaError::source_too_large;
    return result;
  }
  if (limits.maximum_source_bytes == 0) {
    result.error = MediaError::source_too_large;
    return result;
  }

  const auto sector_count = inspection.size_bytes / kSectorSize;
  if (inspection.size_bytes % kSectorSize != 0 ||
      sector_count <= kAnchorSector) {
    result.error = MediaError::too_small;
    return result;
  }

  bool read_failed = false;
  if (!has_udf_102_recognition_sequence(source, read_failed)) {
    result.error = phase_error(
        source, read_failed ? MediaError::io_error
                            : MediaError::unsupported_filesystem);
    return result;
  }

  Sector anchor{};
  if (!read_sector(source, kAnchorSector, anchor)) {
    result.error = phase_error(source, MediaError::io_error);
    return result;
  }
  if (!valid_descriptor_tag(anchor, 2,
                            static_cast<std::uint32_t>(kAnchorSector))) {
    result.error = phase_error(source, MediaError::invalid_structure);
    return result;
  }

  const std::span<const std::byte> anchor_bytes{anchor};
  const auto descriptor_bytes = read_little_u32(anchor_bytes, 16);
  const auto descriptor_sector = read_little_u32(anchor_bytes, 20);
  if (!extent_is_in_bounds(descriptor_bytes, descriptor_sector,
                           sector_count)) {
    result.error = phase_error(source, MediaError::invalid_structure);
    return result;
  }
  if (!inspect_volume_descriptor_sequence(
          source, descriptor_bytes, descriptor_sector,
          inspection.volume_label, read_failed)) {
    result.error = phase_error(
        source, read_failed ? MediaError::io_error
                            : MediaError::invalid_structure);
    return result;
  }

  result.error = verify_source(source);
  if (result.error != MediaError::none) {
    return result;
  }

  inspection.filesystem = "ECMA-167 NSR02 candidate";
  if (!fingerprint_source(source, inspection.size_bytes,
                          limits.maximum_source_bytes,
                          inspection.source_sha256)) {
    result.error = phase_error(source, MediaError::io_error);
    return result;
  }

  result.error = verify_source(source);
  if (result.error != MediaError::none) {
    return result;
  }

  SourceFingerprint fingerprint{
      .size_bytes = inspection.size_bytes,
      .sha256 = inspection.source_sha256,
  };
  result.media = ValidatedMedia{std::move(source), std::move(inspection),
                                std::move(fingerprint)};
  return result;
}

}  // namespace ohl::media
