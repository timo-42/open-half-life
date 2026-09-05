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

// ---------------------------------------------------------------------------
// Independently authored synthetic ECMA-119 (ISO 9660) / Joliet fixtures.
//
// Every byte below is written from the public ECMA-119 structure and the
// public Joliet escape-sequence definition. No name, layout, count or byte
// originates from any real game medium.
// ---------------------------------------------------------------------------

inline constexpr std::size_t kIso9660SectorCount = 300;
inline constexpr std::uint32_t kIsoPrimaryDescriptorSector = 16;
inline constexpr std::uint32_t kIsoPathTableSector = 19;
inline constexpr std::uint32_t kIsoPrimaryRootSector = 24;
inline constexpr std::uint32_t kIsoPrimaryChildSector = 25;
inline constexpr std::uint32_t kIsoJolietRootSector = 26;
inline constexpr std::uint32_t kIsoJolietChildSector = 27;
inline constexpr std::uint32_t kIsoSentinelDataSector = 30;
inline constexpr std::uint32_t kIsoNestedDataSector = 31;

inline constexpr std::string_view kIsoVolumeLabel{"OHL SYNTHETIC"};
inline constexpr std::string_view kIsoPrimaryDirectoryName{"FIXDIR"};
inline constexpr std::string_view kIsoPrimarySentinelName{"SENTINEL.TXT"};
inline constexpr std::string_view kIsoPrimaryNestedName{"NESTED.BIN"};
inline constexpr std::string_view kIsoPrimaryLoopName{"LOOPDIR"};
inline constexpr std::string_view kIsoJolietDirectoryName{"FixtureDir"};
inline constexpr std::string_view kIsoJolietSentinelName{"Sentinel.txt"};
inline constexpr std::string_view kIsoJolietNestedName{"Nested.bin"};
inline constexpr std::string_view kIsoJolietLoopName{"LoopDir"};
inline constexpr std::string_view kIsoSentinelContents{
    "open-half-life synthetic sentinel payload\n"};
inline constexpr std::string_view kIsoNestedContents{
    "open-half-life synthetic nested payload\n"};

struct SyntheticIso9660Options {
  std::size_t sector_count{kIso9660SectorCount};
  bool joliet{true};
  bool terminator{true};
  std::uint16_t logical_block_size{2'048};
  bool volume_space_too_large{false};
  bool root_extent_outside_volume{false};
  bool file_extent_outside_volume{false};
  bool directory_cycle{false};
  bool overlong_identifier{false};
  bool multi_extent_file{false};
  std::uint32_t extra_root_files{0};
  // A file record claiming to live on another volume of the volume set.
  std::uint16_t file_record_volume_sequence{1};
  // Non-zero replaces the root directory record's data length, which
  // ECMA-119 9.1.4 requires to be a whole number of logical blocks.
  std::uint32_t root_size_override{0};
  // Non-zero replaces the child directory record's data length.
  std::uint32_t child_directory_size_override{0};
  // Writes an additional, deliberately malformed supplementary descriptor
  // that carries no Joliet escape sequence.
  bool malformed_non_joliet_supplementary{false};
  // Adds two Joliet siblings differing only in ASCII case.
  bool joliet_case_siblings{false};
};

inline void iso_write_u8(std::vector<std::byte>& image,
                         const std::size_t offset, const std::uint8_t value) {
  image.at(offset) = static_cast<std::byte>(value);
}

inline void iso_write_both_u16(std::vector<std::byte>& image,
                               const std::size_t offset,
                               const std::uint16_t value) {
  iso_write_u8(image, offset, static_cast<std::uint8_t>(value & 0xffU));
  iso_write_u8(image, offset + 1,
               static_cast<std::uint8_t>((value >> 8U) & 0xffU));
  iso_write_u8(image, offset + 2,
               static_cast<std::uint8_t>((value >> 8U) & 0xffU));
  iso_write_u8(image, offset + 3, static_cast<std::uint8_t>(value & 0xffU));
}

inline void iso_write_little_u32(std::vector<std::byte>& image,
                                 const std::size_t offset,
                                 const std::uint32_t value) {
  for (std::size_t index = 0; index < 4; ++index) {
    iso_write_u8(image, offset + index,
                 static_cast<std::uint8_t>((value >> (8U * index)) & 0xffU));
  }
}

inline void iso_write_big_u32(std::vector<std::byte>& image,
                              const std::size_t offset,
                              const std::uint32_t value) {
  for (std::size_t index = 0; index < 4; ++index) {
    iso_write_u8(image, offset + index,
                 static_cast<std::uint8_t>(
                     (value >> (8U * (3U - index))) & 0xffU));
  }
}

inline void iso_write_both_u32(std::vector<std::byte>& image,
                               const std::size_t offset,
                               const std::uint32_t value) {
  iso_write_little_u32(image, offset, value);
  iso_write_big_u32(image, offset + 4, value);
}

inline void iso_write_bytes(std::vector<std::byte>& image,
                            const std::size_t offset,
                            const std::span<const std::byte> value) {
  for (std::size_t index = 0; index < value.size(); ++index) {
    image.at(offset + index) = value[index];
  }
}

inline void iso_write_ascii(std::vector<std::byte>& image,
                            const std::size_t offset,
                            const std::string_view value) {
  for (std::size_t index = 0; index < value.size(); ++index) {
    iso_write_u8(image, offset + index,
                 static_cast<std::uint8_t>(value[index]));
  }
}

inline void iso_fill_ascii_field(std::vector<std::byte>& image,
                                 const std::size_t offset,
                                 const std::size_t size,
                                 const std::string_view value) {
  for (std::size_t index = 0; index < size; ++index) {
    iso_write_u8(
        image, offset + index,
        static_cast<std::uint8_t>(index < value.size()
                                      ? static_cast<std::uint8_t>(value[index])
                                      : 0x20U));
  }
}

// UCS-2 big-endian field padded with U+0020, as Joliet requires.
inline void iso_fill_ucs2_field(std::vector<std::byte>& image,
                                const std::size_t offset,
                                const std::size_t size,
                                const std::string_view value) {
  for (std::size_t index = 0; index * 2U + 1U < size; ++index) {
    const auto character =
        static_cast<std::uint8_t>(index < value.size()
                                      ? static_cast<std::uint8_t>(value[index])
                                      : 0x20U);
    iso_write_u8(image, offset + index * 2U, 0);
    iso_write_u8(image, offset + index * 2U + 1U, character);
  }
}

[[nodiscard]] inline std::vector<std::byte> iso_identifier(
    const std::string_view name, const bool ucs2) {
  std::vector<std::byte> encoded;
  if (ucs2) {
    encoded.reserve(name.size() * 2U);
    for (const auto character : name) {
      encoded.push_back(std::byte{0});
      encoded.push_back(static_cast<std::byte>(
          static_cast<unsigned char>(character)));
    }
  } else {
    encoded.reserve(name.size());
    for (const auto character : name) {
      encoded.push_back(static_cast<std::byte>(
          static_cast<unsigned char>(character)));
    }
  }
  return encoded;
}

struct SyntheticIsoRecord {
  std::vector<std::byte> identifier;
  std::uint32_t extent{0};
  std::uint32_t size{0};
  bool directory{false};
  std::uint8_t extra_flags{0};
  std::uint8_t declared_identifier_length_override{0};
  std::uint16_t volume_sequence{1};
};

// Writes one ECMA-119 directory record and returns its recorded length.
inline std::size_t iso_write_record(std::vector<std::byte>& image,
                                    const std::size_t offset,
                                    const SyntheticIsoRecord& record) {
  const auto identifier_length = record.identifier.size();
  auto length = 33U + identifier_length;
  if ((length % 2U) != 0U) {
    ++length;
  }
  iso_write_u8(image, offset, static_cast<std::uint8_t>(length));
  iso_write_u8(image, offset + 1, 0);
  iso_write_both_u32(image, offset + 2, record.extent);
  iso_write_both_u32(image, offset + 10, record.size);
  iso_write_u8(image, offset + 18, 98);  // years since 1900
  iso_write_u8(image, offset + 19, 1);
  iso_write_u8(image, offset + 20, 1);
  iso_write_u8(image, offset + 25,
               static_cast<std::uint8_t>(
                   (record.directory ? 0x02U : 0x00U) | record.extra_flags));
  iso_write_u8(image, offset + 26, 0);
  iso_write_u8(image, offset + 27, 0);
  iso_write_both_u16(image, offset + 28, record.volume_sequence);
  iso_write_u8(image, offset + 32,
               record.declared_identifier_length_override != 0
                   ? record.declared_identifier_length_override
                   : static_cast<std::uint8_t>(identifier_length));
  iso_write_bytes(image, offset + 33, record.identifier);
  return length;
}

inline void iso_write_directory(std::vector<std::byte>& image,
                                const std::uint32_t sector,
                                const std::uint32_t parent_sector,
                                const std::vector<SyntheticIsoRecord>& records,
                                const bool ucs2) {
  const auto base = static_cast<std::size_t>(sector) * kSyntheticSectorSize;
  std::size_t offset = base;
  SyntheticIsoRecord self;
  self.identifier = {std::byte{0x00}};
  self.extent = sector;
  self.size = static_cast<std::uint32_t>(kSyntheticSectorSize);
  self.directory = true;
  offset += iso_write_record(image, offset, self);
  SyntheticIsoRecord parent = self;
  parent.identifier = {std::byte{0x01}};
  parent.extent = parent_sector;
  offset += iso_write_record(image, offset, parent);
  (void)ucs2;
  for (const auto& record : records) {
    offset += iso_write_record(image, offset, record);
  }
}

inline void iso_write_volume_descriptor(
    std::vector<std::byte>& image, const std::uint32_t sector,
    const std::uint8_t type, const SyntheticIso9660Options& options,
    const std::uint32_t volume_blocks, const std::uint32_t root_sector,
    const bool joliet) {
  const auto base = static_cast<std::size_t>(sector) * kSyntheticSectorSize;
  iso_write_u8(image, base, type);
  iso_write_ascii(image, base + 1, "CD001");
  iso_write_u8(image, base + 6, 1);
  iso_fill_ascii_field(image, base + 8, 32, "");
  if (joliet) {
    iso_fill_ucs2_field(image, base + 40, 32, kIsoVolumeLabel);
    iso_write_u8(image, base + 88, 0x25U);
    iso_write_u8(image, base + 89, 0x2fU);
    iso_write_u8(image, base + 90, 0x45U);
  } else {
    iso_fill_ascii_field(image, base + 40, 32, kIsoVolumeLabel);
  }
  iso_write_both_u32(image, base + 80,
                     options.volume_space_too_large ? volume_blocks + 100U
                                                    : volume_blocks);
  iso_write_both_u16(image, base + 120, 1);
  iso_write_both_u16(image, base + 124, 1);
  iso_write_both_u16(image, base + 128, options.logical_block_size);
  iso_write_both_u32(image, base + 132, 10);
  iso_write_little_u32(image, base + 140, kIsoPathTableSector);
  iso_write_little_u32(image, base + 144, 0);
  iso_write_big_u32(image, base + 148, kIsoPathTableSector);
  iso_write_big_u32(image, base + 152, 0);

  SyntheticIsoRecord root;
  root.identifier = {std::byte{0x00}};
  root.extent = options.root_extent_outside_volume ? volume_blocks + 5U
                                                   : root_sector;
  root.size = options.root_size_override != 0
                  ? options.root_size_override
                  : static_cast<std::uint32_t>(kSyntheticSectorSize);
  root.directory = true;
  (void)iso_write_record(image, base + 156, root);

  iso_fill_ascii_field(image, base + 190, 128, "");
  iso_fill_ascii_field(image, base + 318, 128, "");
  iso_fill_ascii_field(image, base + 446, 128, "");
  iso_write_u8(image, base + 881, 1);
}

// Builds one complete synthetic ECMA-119 image, optionally carrying a Joliet
// supplementary descriptor and optionally one deliberate structural defect.
[[nodiscard]] inline std::vector<std::byte> make_synthetic_iso9660_image(
    const SyntheticIso9660Options options = {}) {
  if (options.sector_count < 64) {
    throw std::invalid_argument{"synthetic ISO 9660 image is too small"};
  }
  std::vector<std::byte> image(options.sector_count * kSyntheticSectorSize);
  const auto volume_blocks =
      static_cast<std::uint32_t>(options.sector_count);

  iso_write_volume_descriptor(image, kIsoPrimaryDescriptorSector, 1, options,
                              volume_blocks, kIsoPrimaryRootSector, false);
  std::uint32_t next_descriptor = kIsoPrimaryDescriptorSector + 1U;
  if (options.joliet) {
    iso_write_volume_descriptor(image, next_descriptor, 2, options,
                                volume_blocks, kIsoJolietRootSector, true);
    ++next_descriptor;
  }
  if (options.malformed_non_joliet_supplementary) {
    SyntheticIso9660Options malformed = options;
    malformed.logical_block_size = 512;
    malformed.root_extent_outside_volume = true;
    iso_write_volume_descriptor(image, next_descriptor, 2, malformed,
                                volume_blocks, kIsoPrimaryRootSector, false);
    ++next_descriptor;
  }
  if (options.terminator) {
    const auto base =
        static_cast<std::size_t>(next_descriptor) * kSyntheticSectorSize;
    iso_write_u8(image, base, 255);
    iso_write_ascii(image, base + 1, "CD001");
    iso_write_u8(image, base + 6, 1);
  }

  // Minimal type-L path table describing only the root directory.
  const auto path_table =
      static_cast<std::size_t>(kIsoPathTableSector) * kSyntheticSectorSize;
  iso_write_u8(image, path_table, 1);
  iso_write_u8(image, path_table + 1, 0);
  iso_write_little_u32(image, path_table + 2, kIsoPrimaryRootSector);
  iso_write_u8(image, path_table + 6, 1);
  iso_write_u8(image, path_table + 8, 0);

  iso_write_ascii(image,
                  static_cast<std::size_t>(kIsoSentinelDataSector) *
                      kSyntheticSectorSize,
                  kIsoSentinelContents);
  iso_write_ascii(image,
                  static_cast<std::size_t>(kIsoNestedDataSector) *
                      kSyntheticSectorSize,
                  kIsoNestedContents);

  const auto build_tree = [&](const bool ucs2, const std::uint32_t root_sector,
                              const std::uint32_t child_sector,
                              const std::string_view directory_name,
                              const std::string_view sentinel_name,
                              const std::string_view nested_name,
                              const std::string_view loop_name) {
    std::vector<SyntheticIsoRecord> root_records;
    SyntheticIsoRecord directory;
    directory.identifier = iso_identifier(directory_name, ucs2);
    directory.extent = child_sector;
    directory.size = options.child_directory_size_override != 0
                         ? options.child_directory_size_override
                         : static_cast<std::uint32_t>(kSyntheticSectorSize);
    directory.directory = true;
    root_records.push_back(std::move(directory));

    SyntheticIsoRecord sentinel;
    sentinel.identifier = iso_identifier(
        std::string{sentinel_name} + ";1", ucs2);
    sentinel.extent = options.file_extent_outside_volume ? volume_blocks + 5U
                                                         : kIsoSentinelDataSector;
    sentinel.size = static_cast<std::uint32_t>(kIsoSentinelContents.size());
    if (options.multi_extent_file) {
      sentinel.extra_flags = 0x80U;
    }
    if (options.overlong_identifier) {
      sentinel.declared_identifier_length_override =
          static_cast<std::uint8_t>(sentinel.identifier.size() + 40U);
    }
    sentinel.volume_sequence = options.file_record_volume_sequence;
    root_records.push_back(std::move(sentinel));

    if (options.joliet_case_siblings && ucs2) {
      for (const auto& sibling :
           {std::pair<std::string_view, std::uint32_t>{"CaseName.txt", 8U},
            std::pair<std::string_view, std::uint32_t>{"casename.txt", 9U}}) {
        SyntheticIsoRecord entry;
        entry.identifier =
            iso_identifier(std::string{sibling.first} + ";1", true);
        entry.extent = kIsoSentinelDataSector;
        entry.size = sibling.second;
        root_records.push_back(std::move(entry));
      }
    }

    for (std::uint32_t index = 0; index < options.extra_root_files; ++index) {
      SyntheticIsoRecord extra;
      const auto name = "EXTRA" + std::to_string(index) + ".TXT;1";
      extra.identifier = iso_identifier(name, ucs2);
      extra.extent = kIsoSentinelDataSector;
      extra.size = 8;
      root_records.push_back(std::move(extra));
    }
    iso_write_directory(image, root_sector, root_sector, root_records, ucs2);

    std::vector<SyntheticIsoRecord> child_records;
    SyntheticIsoRecord nested;
    nested.identifier = iso_identifier(std::string{nested_name} + ";1", ucs2);
    nested.extent = kIsoNestedDataSector;
    nested.size = static_cast<std::uint32_t>(kIsoNestedContents.size());
    child_records.push_back(std::move(nested));
    if (options.directory_cycle) {
      SyntheticIsoRecord loop;
      loop.identifier = iso_identifier(loop_name, ucs2);
      loop.extent = root_sector;
      loop.size = static_cast<std::uint32_t>(kSyntheticSectorSize);
      loop.directory = true;
      child_records.push_back(std::move(loop));
    }
    iso_write_directory(image, child_sector, root_sector, child_records, ucs2);
  };

  build_tree(false, kIsoPrimaryRootSector, kIsoPrimaryChildSector,
             kIsoPrimaryDirectoryName, kIsoPrimarySentinelName,
             kIsoPrimaryNestedName, kIsoPrimaryLoopName);
  if (options.joliet) {
    build_tree(true, kIsoJolietRootSector, kIsoJolietChildSector,
               kIsoJolietDirectoryName, kIsoJolietSentinelName,
               kIsoJolietNestedName, kIsoJolietLoopName);
  }
  return image;
}

}  // namespace ohl::media::test
