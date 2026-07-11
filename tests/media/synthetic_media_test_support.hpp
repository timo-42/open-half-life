#pragma once

#include "ohl/media/iso_inspector.hpp"

#include <atomic>
#include <chrono>
#include <cstddef>
#include <cstdint>
#include <filesystem>
#include <fstream>
#include <span>
#include <stdexcept>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace ohl::media::test {

inline constexpr std::size_t kSyntheticSectorSize = 2'048;
inline constexpr std::size_t kSyntheticMinimumSectorCount = 300;
using SyntheticSector = std::span<std::byte, kSyntheticSectorSize>;

inline void write_little_u16(const SyntheticSector sector,
                             const std::size_t offset,
                             const std::uint16_t value) {
  sector[offset] = static_cast<std::byte>(value & 0xffU);
  sector[offset + 1] = static_cast<std::byte>((value >> 8U) & 0xffU);
}

inline void write_little_u32(const SyntheticSector sector,
                             const std::size_t offset,
                             const std::uint32_t value) {
  sector[offset] = static_cast<std::byte>(value & 0xffU);
  sector[offset + 1] = static_cast<std::byte>((value >> 8U) & 0xffU);
  sector[offset + 2] = static_cast<std::byte>((value >> 16U) & 0xffU);
  sector[offset + 3] = static_cast<std::byte>((value >> 24U) & 0xffU);
}

[[nodiscard]] inline std::uint16_t synthetic_crc_itu_t(
    const std::span<const std::byte> bytes) {
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

inline void finish_synthetic_tag(const SyntheticSector sector,
                                 const std::uint16_t identifier,
                                 const std::uint32_t location,
                                 const std::uint16_t crc_length = 496) {
  write_little_u16(sector, 0, identifier);
  write_little_u16(sector, 2, 2);
  write_little_u16(sector, 6, 1);
  write_little_u16(sector, 10, crc_length);
  write_little_u32(sector, 12, location);
  write_little_u16(
      sector, 8,
      synthetic_crc_itu_t(
          std::span<const std::byte>{sector}.subspan(16, crc_length)));

  std::uint8_t checksum = 0;
  for (std::size_t index = 0; index < 16; ++index) {
    if (index != 4) {
      checksum = static_cast<std::uint8_t>(
          checksum + std::to_integer<std::uint8_t>(sector[index]));
    }
  }
  sector[4] = static_cast<std::byte>(checksum);
}

inline void set_synthetic_identifier(const SyntheticSector sector,
                                     const std::string_view identifier) {
  sector[0] = std::byte{0};
  for (std::size_t index = 0; index < identifier.size(); ++index) {
    sector[index + 1] = static_cast<std::byte>(identifier[index]);
  }
  sector[6] = std::byte{1};
}

[[nodiscard]] inline SyntheticSector synthetic_sector_at(
    std::vector<std::byte>& image, const std::size_t index) {
  return SyntheticSector{image.data() + index * kSyntheticSectorSize,
                         kSyntheticSectorSize};
}

// Independently authored minimal ECMA-167 NSR02-shaped validation fixture.
// The marker lives outside every descriptor and is useful for digest variants.
[[nodiscard]] inline std::vector<std::byte> make_synthetic_validated_image(
    const std::size_t sector_count = kSyntheticMinimumSectorCount,
    const std::byte marker = std::byte{0}) {
  if (sector_count < kSyntheticMinimumSectorCount) {
    throw std::invalid_argument{"synthetic media is too small"};
  }
  std::vector<std::byte> image(sector_count * kSyntheticSectorSize);
  set_synthetic_identifier(synthetic_sector_at(image, 18), "BEA01");
  set_synthetic_identifier(synthetic_sector_at(image, 19), "NSR02");
  set_synthetic_identifier(synthetic_sector_at(image, 20), "TEA01");

  auto primary = synthetic_sector_at(image, 32);
  constexpr std::string_view volume_label{"PROJECT SYNTHETIC"};
  primary[24] = std::byte{8};
  for (std::size_t index = 0; index < volume_label.size(); ++index) {
    primary[25 + index] = static_cast<std::byte>(volume_label[index]);
  }
  primary[55] = static_cast<std::byte>(volume_label.size() + 1);
  finish_synthetic_tag(primary, 1, 32);

  finish_synthetic_tag(synthetic_sector_at(image, 33), 5, 33);
  auto logical = synthetic_sector_at(image, 34);
  write_little_u32(logical, 212, 2'048);
  finish_synthetic_tag(logical, 6, 34, 424);
  finish_synthetic_tag(synthetic_sector_at(image, 35), 8, 35);

  auto anchor = synthetic_sector_at(image, 256);
  write_little_u32(anchor, 16, 16 * 2'048);
  write_little_u32(anchor, 20, 32);
  write_little_u32(anchor, 24, 16 * 2'048);
  write_little_u32(anchor, 28, 48);
  finish_synthetic_tag(anchor, 2, 256);
  image[100 * kSyntheticSectorSize] = marker;
  return image;
}

class SyntheticValidatedMedia final {
 public:
  explicit SyntheticValidatedMedia(
      const std::size_t sector_count = kSyntheticMinimumSectorCount,
      const std::byte marker = std::byte{0}) {
    static std::atomic<std::uint64_t> sequence{0};
    const auto id = sequence.fetch_add(1, std::memory_order_relaxed);
    directory_ = std::filesystem::temp_directory_path() /
                 ("ohl-project-synthetic-media-" +
                  std::to_string(std::chrono::steady_clock::now()
                                     .time_since_epoch()
                                     .count()) +
                  "-" + std::to_string(id));
    path_ = directory_ / "validated-media.bin";
    std::error_code error;
    std::filesystem::create_directories(directory_, error);
    if (error || !write_image(make_synthetic_validated_image(sector_count,
                                                              marker))) {
      throw std::runtime_error{"failed to create synthetic media"};
    }
    auto opened = ohl::platform::open_media_source(path_);
    if (!opened.valid()) {
      throw std::runtime_error{"failed to pin synthetic media"};
    }
    validation_ = ohl::media::validate_iso(std::move(opened.source));
    if (!validation_.valid()) {
      throw std::runtime_error{"failed to validate synthetic media"};
    }
  }

  ~SyntheticValidatedMedia() {
    validation_.media.reset();
    std::error_code ignored;
    std::filesystem::remove_all(directory_, ignored);
  }

  SyntheticValidatedMedia(const SyntheticValidatedMedia&) = delete;
  SyntheticValidatedMedia& operator=(const SyntheticValidatedMedia&) = delete;

  [[nodiscard]] const ohl::media::ValidatedMedia& media() const {
    return *validation_.media;
  }

  [[nodiscard]] ohl::media::ValidatedMedia& media() {
    return *validation_.media;
  }

  [[nodiscard]] const std::filesystem::path& path() const noexcept {
    return path_;
  }

  [[nodiscard]] bool overwrite_byte(const std::uint64_t offset,
                                    const std::byte value,
                                    const bool restore_write_time) {
    std::error_code error;
    const auto original_time = std::filesystem::last_write_time(path_, error);
    if (error) {
      return false;
    }
    std::fstream file{path_, std::ios::in | std::ios::out | std::ios::binary};
    file.seekp(static_cast<std::streamoff>(offset));
    const auto byte = static_cast<char>(std::to_integer<unsigned char>(value));
    file.write(&byte, 1);
    file.flush();
    if (!file.good()) {
      return false;
    }
    file.close();
    std::filesystem::last_write_time(
        path_, restore_write_time ? original_time
                                  : original_time - std::chrono::seconds{2},
        error);
    return !error;
  }

 private:
  [[nodiscard]] bool write_image(const std::vector<std::byte>& image) const {
    std::ofstream output{path_, std::ios::binary};
    output.write(reinterpret_cast<const char*>(image.data()),
                 static_cast<std::streamsize>(image.size()));
    return output.good();
  }

  std::filesystem::path directory_;
  std::filesystem::path path_;
  ohl::media::IsoValidationResult validation_;
};

}  // namespace ohl::media::test
