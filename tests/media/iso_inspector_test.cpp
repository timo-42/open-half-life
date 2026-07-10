#include "ohl/media/iso_inspector.hpp"

#include <algorithm>
#include <array>
#include <cstddef>
#include <cstdint>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <span>
#include <string>
#include <vector>

namespace {

constexpr std::size_t kSectorSize = 2'048;
constexpr std::size_t kSectorCount = 300;
using Sector = std::span<std::byte, kSectorSize>;

void write_little_u16(const Sector sector, const std::size_t offset,
                      const std::uint16_t value) {
  sector[offset] = static_cast<std::byte>(value & 0xffU);
  sector[offset + 1] = static_cast<std::byte>((value >> 8U) & 0xffU);
}

void write_little_u32(const Sector sector, const std::size_t offset,
                      const std::uint32_t value) {
  sector[offset] = static_cast<std::byte>(value & 0xffU);
  sector[offset + 1] = static_cast<std::byte>((value >> 8U) & 0xffU);
  sector[offset + 2] = static_cast<std::byte>((value >> 16U) & 0xffU);
  sector[offset + 3] = static_cast<std::byte>((value >> 24U) & 0xffU);
}

[[nodiscard]] std::uint16_t crc_itu_t(
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

void finish_tag(const Sector sector, const std::uint16_t identifier,
                const std::uint32_t location,
                const std::uint16_t crc_length = 496) {
  write_little_u16(sector, 0, identifier);
  write_little_u16(sector, 2, 2);
  write_little_u16(sector, 6, 1);
  write_little_u16(sector, 10, crc_length);
  write_little_u32(sector, 12, location);
  write_little_u16(sector, 8,
                   crc_itu_t(std::span<const std::byte>{sector}.subspan(
                       16, crc_length)));

  std::uint8_t checksum = 0;
  for (std::size_t index = 0; index < 16; ++index) {
    if (index != 4) {
      checksum = static_cast<std::uint8_t>(
          checksum + std::to_integer<std::uint8_t>(sector[index]));
    }
  }
  sector[4] = static_cast<std::byte>(checksum);
}

void set_identifier(const Sector sector, const std::string& identifier) {
  sector[0] = std::byte{0};
  for (std::size_t index = 0; index < identifier.size(); ++index) {
    sector[index + 1] = static_cast<std::byte>(identifier[index]);
  }
  sector[6] = std::byte{1};
}

[[nodiscard]] Sector sector_at(std::vector<std::byte>& image,
                               const std::size_t index) {
  return Sector{image.data() + index * kSectorSize, kSectorSize};
}

[[nodiscard]] std::vector<std::byte> make_valid_image() {
  std::vector<std::byte> image(kSectorCount * kSectorSize);
  set_identifier(sector_at(image, 18), "BEA01");
  set_identifier(sector_at(image, 19), "NSR02");
  set_identifier(sector_at(image, 20), "TEA01");

  auto primary = sector_at(image, 32);
  const std::string volume_label{"SYNTHETIC"};
  primary[24] = std::byte{8};
  for (std::size_t index = 0; index < volume_label.size(); ++index) {
    primary[25 + index] = static_cast<std::byte>(volume_label[index]);
  }
  primary[55] = static_cast<std::byte>(volume_label.size() + 1);
  finish_tag(primary, 1, 32);

  auto partition = sector_at(image, 33);
  finish_tag(partition, 5, 33);

  auto logical = sector_at(image, 34);
  write_little_u32(logical, 212, 2'048);
  finish_tag(logical, 6, 34, 424);

  auto terminator = sector_at(image, 35);
  finish_tag(terminator, 8, 35);

  auto anchor = sector_at(image, 256);
  write_little_u32(anchor, 16, 16 * 2'048);
  write_little_u32(anchor, 20, 32);
  write_little_u32(anchor, 24, 16 * 2'048);
  write_little_u32(anchor, 28, 48);
  finish_tag(anchor, 2, 256);
  return image;
}

[[nodiscard]] bool write_image(const std::filesystem::path& path,
                               const std::vector<std::byte>& image) {
  std::ofstream output{path, std::ios::binary};
  output.write(reinterpret_cast<const char*>(image.data()),
               static_cast<std::streamsize>(image.size()));
  return output.good();
}

int expect_error(const ohl::media::MediaError actual,
                 const ohl::media::MediaError expected,
                 const std::string& context) {
  if (actual != expected) {
    std::cerr << context << ": expected " << ohl::media::to_string(expected)
              << ", got " << ohl::media::to_string(actual) << '\n';
    return 1;
  }
  return 0;
}

}  // namespace

int main() {
  const auto directory = std::filesystem::temp_directory_path() /
                         "open-half-life-media-tests";
  std::error_code error_code;
  std::filesystem::remove_all(directory, error_code);
  std::filesystem::create_directories(directory, error_code);
  if (error_code) {
    std::cerr << "failed to create temporary test directory\n";
    return 1;
  }

  const auto missing = ohl::media::inspect_iso(directory / "missing.iso");
  if (expect_error(missing.error, ohl::media::MediaError::not_found,
                   "missing file") != 0) {
    return 1;
  }

  const auto directory_result = ohl::media::inspect_iso(directory);
  if (expect_error(directory_result.error,
                   ohl::media::MediaError::not_regular_file,
                   "directory") != 0) {
    return 1;
  }

  const auto too_small_path = directory / "too-small.iso";
  if (!write_image(too_small_path, std::vector<std::byte>(128))) {
    std::cerr << "failed to write too-small synthetic image\n";
    return 1;
  }
  const auto too_small = ohl::media::inspect_iso(too_small_path);
  if (expect_error(too_small.error, ohl::media::MediaError::too_small,
                   "too-small image") != 0) {
    return 1;
  }

  const auto valid_path = directory / "valid.iso";
  auto valid_image = make_valid_image();
  if (!write_image(valid_path, valid_image)) {
    std::cerr << "failed to write synthetic image\n";
    return 1;
  }
  const auto valid = ohl::media::inspect_iso(valid_path);
  if (!valid.valid() || valid.filesystem != "ECMA-167 NSR02 candidate" ||
      valid.volume_label != "SYNTHETIC") {
    std::cerr << "valid synthetic ECMA-167 candidate was rejected: "
              << ohl::media::to_string(valid.error) << '\n';
    return 1;
  }

  valid_image[256 * kSectorSize + 4] ^= std::byte{1};
  const auto corrupt_path = directory / "corrupt.iso";
  if (!write_image(corrupt_path, valid_image)) {
    std::cerr << "failed to write corrupt synthetic image\n";
    return 1;
  }
  const auto corrupt = ohl::media::inspect_iso(corrupt_path);
  if (expect_error(corrupt.error, ohl::media::MediaError::invalid_structure,
                   "corrupt anchor") != 0) {
    return 1;
  }

  auto zero_crc_image = make_valid_image();
  finish_tag(sector_at(zero_crc_image, 256), 2, 256, 0);
  const auto zero_crc_path = directory / "zero-crc.iso";
  if (!write_image(zero_crc_path, zero_crc_image)) {
    std::cerr << "failed to write zero-CRC synthetic image\n";
    return 1;
  }
  const auto zero_crc = ohl::media::inspect_iso(zero_crc_path);
  if (expect_error(zero_crc.error,
                   ohl::media::MediaError::invalid_structure,
                   "zero-length descriptor CRC") != 0) {
    return 1;
  }

  auto out_of_bounds_image = make_valid_image();
  auto out_of_bounds_anchor = sector_at(out_of_bounds_image, 256);
  write_little_u32(out_of_bounds_anchor, 20,
                   static_cast<std::uint32_t>(kSectorCount + 1));
  finish_tag(out_of_bounds_anchor, 2, 256);
  const auto out_of_bounds_path = directory / "out-of-bounds.iso";
  if (!write_image(out_of_bounds_path, out_of_bounds_image)) {
    std::cerr << "failed to write out-of-bounds synthetic image\n";
    return 1;
  }
  const auto out_of_bounds = ohl::media::inspect_iso(out_of_bounds_path);
  if (expect_error(out_of_bounds.error,
                   ohl::media::MediaError::invalid_structure,
                   "out-of-bounds extent") != 0) {
    return 1;
  }

  auto missing_descriptor_image = make_valid_image();
  std::fill(sector_at(missing_descriptor_image, 33).begin(),
            sector_at(missing_descriptor_image, 33).end(), std::byte{0});
  const auto missing_descriptor_path = directory / "missing-descriptor.iso";
  if (!write_image(missing_descriptor_path, missing_descriptor_image)) {
    std::cerr << "failed to write missing-descriptor synthetic image\n";
    return 1;
  }
  const auto missing_descriptor =
      ohl::media::inspect_iso(missing_descriptor_path);
  if (expect_error(missing_descriptor.error,
                   ohl::media::MediaError::invalid_structure,
                   "missing required descriptor") != 0) {
    return 1;
  }

  auto truncated_image = make_valid_image();
  truncated_image.pop_back();
  const auto truncated_path = directory / "truncated.iso";
  if (!write_image(truncated_path, truncated_image)) {
    std::cerr << "failed to write truncated synthetic image\n";
    return 1;
  }
  const auto truncated = ohl::media::inspect_iso(truncated_path);
  if (expect_error(truncated.error, ohl::media::MediaError::too_small,
                   "non-sector-aligned image") != 0) {
    return 1;
  }

  const auto random_path = directory / "random.iso";
  const std::vector<std::byte> random_image(kSectorCount * kSectorSize);
  if (!write_image(random_path, random_image)) {
    std::cerr << "failed to write random synthetic image\n";
    return 1;
  }
  const auto random = ohl::media::inspect_iso(random_path);
  if (expect_error(random.error,
                   ohl::media::MediaError::unsupported_filesystem,
                   "random data") != 0) {
    return 1;
  }

  std::filesystem::remove_all(directory, error_code);
  return 0;
}
