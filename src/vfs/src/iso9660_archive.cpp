#include "ohl/vfs/iso9660_archive.hpp"

#include "ohl/platform/media_source.hpp"
#include "udf_archive_internal.hpp"

#include <algorithm>
#include <array>
#include <cstddef>
#include <cstdint>
#include <iterator>
#include <limits>
#include <mutex>
#include <optional>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

// Project-owned, clean-room ECMA-119 (ISO 9660) reader.
//
// Structure and field offsets come only from the public ECMA-119 standard and
// Microsoft's public Joliet specification. Every recorded value is range
// checked against the mounted volume geometry before it is used, both-endian
// pairs must agree, and no allocation is sized by an untrusted field: sectors
// are read one at a time into a fixed buffer and decoded names are capped.
//
// Nothing in this file logs or returns media-derived diagnostics.

namespace ohl::vfs {
namespace {

constexpr std::uint64_t kBlockSize = UdfArchiveLimits::logical_block_size;
constexpr std::uint64_t kFirstDescriptorSector = 16;
constexpr std::uint64_t kDescriptorScanLimit = 32;
constexpr std::size_t kDirectoryRecordBytes = 34;
constexpr std::size_t kFileIdentifierOffset = 33;
constexpr std::uint8_t kTypePrimary = 1;
constexpr std::uint8_t kTypeSupplementary = 2;
constexpr std::uint8_t kTypeTerminator = 255;
constexpr std::uint8_t kFlagDirectory = 0x02;
constexpr std::uint8_t kFlagAssociated = 0x04;
constexpr std::uint8_t kFlagMultiExtent = 0x80;

using Block = std::array<std::byte, kBlockSize>;

[[nodiscard]] std::uint8_t byte_at(const std::span<const std::byte> bytes,
                                   const std::size_t offset) noexcept {
  return std::to_integer<std::uint8_t>(bytes[offset]);
}

[[nodiscard]] std::uint32_t read_little_u32(
    const std::span<const std::byte> bytes,
    const std::size_t offset) noexcept {
  return static_cast<std::uint32_t>(byte_at(bytes, offset)) |
         (static_cast<std::uint32_t>(byte_at(bytes, offset + 1)) << 8U) |
         (static_cast<std::uint32_t>(byte_at(bytes, offset + 2)) << 16U) |
         (static_cast<std::uint32_t>(byte_at(bytes, offset + 3)) << 24U);
}

[[nodiscard]] std::uint32_t read_big_u32(
    const std::span<const std::byte> bytes,
    const std::size_t offset) noexcept {
  return static_cast<std::uint32_t>(byte_at(bytes, offset + 3)) |
         (static_cast<std::uint32_t>(byte_at(bytes, offset + 2)) << 8U) |
         (static_cast<std::uint32_t>(byte_at(bytes, offset + 1)) << 16U) |
         (static_cast<std::uint32_t>(byte_at(bytes, offset)) << 24U);
}

// ECMA-119 records numeric values twice, little-endian first. A disagreement
// is a structural error rather than a hint to prefer one order.
[[nodiscard]] bool read_both_endian_u32(
    const std::span<const std::byte> bytes, const std::size_t offset,
    std::uint32_t& value) noexcept {
  value = read_little_u32(bytes, offset);
  return value == read_big_u32(bytes, offset + 4);
}

[[nodiscard]] bool read_both_endian_u16(
    const std::span<const std::byte> bytes, const std::size_t offset,
    std::uint16_t& value) noexcept {
  value = static_cast<std::uint16_t>(
      static_cast<std::uint16_t>(byte_at(bytes, offset)) |
      static_cast<std::uint16_t>(
          static_cast<std::uint16_t>(byte_at(bytes, offset + 1)) << 8U));
  const auto big = static_cast<std::uint16_t>(
      static_cast<std::uint16_t>(byte_at(bytes, offset + 3)) |
      static_cast<std::uint16_t>(
          static_cast<std::uint16_t>(byte_at(bytes, offset + 2)) << 8U));
  return value == big;
}

[[nodiscard]] std::uint64_t sectors_for(const std::uint64_t bytes) noexcept {
  return (bytes + kBlockSize - 1U) / kBlockSize;
}

struct IsoState {
  IsoState(SharedMediaSource input_source, const UdfArchiveLimits input_limits)
      : source{std::move(input_source)}, limits{input_limits} {}

  SharedMediaSource source;
  const UdfArchiveLimits limits;
  std::uint32_t volume_blocks{0};
  std::uint16_t volume_set_size{1};
  std::uint32_t root_extent{0};
  std::uint32_t root_size{0};
  bool joliet{false};
  std::string label;
  mutable std::recursive_mutex mutex;
};

[[nodiscard]] VfsError map_source_error(
    const platform::MediaSourceError error) noexcept {
  switch (error) {
    case platform::MediaSourceError::none:
      return VfsError::none;
    case platform::MediaSourceError::changed:
      return VfsError::source_changed;
    case platform::MediaSourceError::resource_exhausted:
      return VfsError::limit_exceeded;
    case platform::MediaSourceError::out_of_range:
    case platform::MediaSourceError::not_found:
    case platform::MediaSourceError::not_regular_file:
    case platform::MediaSourceError::open_failed:
    case platform::MediaSourceError::read_failed:
    case platform::MediaSourceError::unexpected_eof:
    case platform::MediaSourceError::unsupported:
      return VfsError::read_failed;
  }
  return VfsError::read_failed;
}

[[nodiscard]] VfsError read_block(const SharedMediaSource& source,
                                  const std::uint64_t sector,
                                  Block& destination) noexcept {
  if (source == nullptr ||
      sector > std::numeric_limits<std::uint64_t>::max() / kBlockSize) {
    return VfsError::invalid_source;
  }
  return map_source_error(
      source->read_exact_at(sector * kBlockSize, destination));
}

[[nodiscard]] std::string sanitized_label(const std::string_view value) {
  std::string result;
  result.reserve(value.size());
  for (const auto character : value) {
    const auto byte = static_cast<unsigned char>(character);
    result.push_back(byte >= 0x20U && byte <= 0x7eU ? character : '?');
  }
  while (!result.empty() && result.back() == ' ') {
    result.pop_back();
  }
  return result;
}

// --- Volume descriptors ----------------------------------------------------

struct DescriptorFields {
  std::uint32_t volume_blocks{0};
  std::uint16_t volume_set_size{1};
  std::uint32_t root_extent{0};
  std::uint32_t root_size{0};
  bool joliet{false};
  std::string label;
};

[[nodiscard]] bool is_descriptor(const std::span<const std::byte> bytes,
                                 std::uint8_t& type) noexcept {
  type = byte_at(bytes, 0);
  return byte_at(bytes, 1) == 'C' && byte_at(bytes, 2) == 'D' &&
         byte_at(bytes, 3) == '0' && byte_at(bytes, 4) == '0' &&
         byte_at(bytes, 5) == '1' && byte_at(bytes, 6) == 1;
}

// Joliet reserves the UCS-2 level 1/2/3 escape sequences %/@, %/C and %/E in
// the supplementary volume descriptor.
[[nodiscard]] bool has_joliet_escape(
    const std::span<const std::byte> bytes) noexcept {
  if (byte_at(bytes, 88) != 0x25U || byte_at(bytes, 89) != 0x2fU) {
    return false;
  }
  const auto level = byte_at(bytes, 90);
  return level == 0x40U || level == 0x43U || level == 0x45U;
}

[[nodiscard]] std::string decode_label(const std::span<const std::byte> bytes,
                                       const bool ucs2) {
  std::string decoded;
  if (ucs2) {
    for (std::size_t index = 40; index + 1 < 72; index += 2) {
      const auto high = byte_at(bytes, index);
      const auto low = byte_at(bytes, index + 1);
      decoded.push_back(high == 0 ? static_cast<char>(low) : '?');
    }
  } else {
    for (std::size_t index = 40; index < 72; ++index) {
      decoded.push_back(static_cast<char>(byte_at(bytes, index)));
    }
  }
  return sanitized_label(decoded);
}

// This validator is an intentional, independent second copy of the bounded
// descriptor checks in src/media/src/iso_inspector.cpp: the preflight decides
// whether media may be used at all, and this reader never trusts that result
// as a parsing input. The two must be kept in sync when either changes.
[[nodiscard]] bool parse_volume_descriptor(
    const std::span<const std::byte> bytes, const std::uint64_t sector_count,
    DescriptorFields& fields) noexcept {
  std::uint16_t block_size = 0;
  if (!read_both_endian_u16(bytes, 128, block_size) ||
      block_size != kBlockSize) {
    return false;
  }

  std::uint16_t volume_set_size = 0;
  if (!read_both_endian_u16(bytes, 120, volume_set_size) ||
      volume_set_size == 0) {
    return false;
  }

  std::uint32_t volume_blocks = 0;
  if (!read_both_endian_u32(bytes, 80, volume_blocks) || volume_blocks == 0 ||
      static_cast<std::uint64_t>(volume_blocks) > sector_count) {
    return false;
  }

  std::uint32_t path_table_bytes = 0;
  if (!read_both_endian_u32(bytes, 132, path_table_bytes) ||
      path_table_bytes == 0 ||
      static_cast<std::uint64_t>(path_table_bytes) >
          static_cast<std::uint64_t>(volume_blocks) * kBlockSize) {
    return false;
  }
  const auto path_table_sectors = sectors_for(path_table_bytes);
  const auto table_in_bounds =
      [volume_blocks, path_table_sectors](
          const std::uint32_t location) noexcept {
        return location == 0 ||
               static_cast<std::uint64_t>(location) + path_table_sectors <=
                   volume_blocks;
      };
  const auto type_l = read_little_u32(bytes, 140);
  if (type_l == 0 || !table_in_bounds(type_l) ||
      !table_in_bounds(read_little_u32(bytes, 144))) {
    return false;
  }

  const auto root = bytes.subspan(156, kDirectoryRecordBytes);
  if (byte_at(root, 0) != kDirectoryRecordBytes || byte_at(root, 1) != 0 ||
      (byte_at(root, 25) & kFlagDirectory) == 0 ||
      (byte_at(root, 25) & kFlagMultiExtent) != 0 || byte_at(root, 26) != 0 ||
      byte_at(root, 27) != 0) {
    return false;
  }
  std::uint32_t root_extent = 0;
  std::uint32_t root_size = 0;
  std::uint16_t root_volume_sequence = 0;
  // ECMA-119 9.1.4: a directory's data length is always a whole number of
  // logical blocks.
  if (!read_both_endian_u32(root, 2, root_extent) ||
      !read_both_endian_u32(root, 10, root_size) || root_size == 0 ||
      (root_size % kBlockSize) != 0 ||
      !read_both_endian_u16(root, 28, root_volume_sequence) ||
      root_volume_sequence != 1 || root_volume_sequence > volume_set_size ||
      sectors_for(root_size) > Iso9660Limits::hard_max_directory_sectors ||
      static_cast<std::uint64_t>(root_extent) + sectors_for(root_size) >
          volume_blocks) {
    return false;
  }

  fields.volume_set_size = volume_set_size;
  fields.volume_blocks = volume_blocks;
  fields.root_extent = root_extent;
  fields.root_size = root_size;
  return true;
}

// Reads the volume descriptor set and selects the Joliet tree when a valid
// supplementary descriptor carries one of the reserved escape sequences.
[[nodiscard]] VfsError mount_descriptors(const SharedMediaSource& source,
                                         const std::uint64_t sector_count,
                                         IsoState& state) {
  Block block{};
  bool found_primary = false;
  bool found_terminator = false;
  DescriptorFields primary;
  DescriptorFields joliet;

  for (std::uint64_t index = 0; index < kDescriptorScanLimit; ++index) {
    const auto location = kFirstDescriptorSector + index;
    if (location >= sector_count) {
      return VfsError::invalid_source;
    }
    const auto read_error = read_block(source, location, block);
    if (read_error != VfsError::none) {
      return read_error;
    }
    const std::span<const std::byte> bytes{block};
    std::uint8_t type = 0;
    if (!is_descriptor(bytes, type)) {
      return VfsError::invalid_source;
    }
    if (type == kTypeTerminator) {
      found_terminator = true;
      break;
    }
    if (type == kTypePrimary) {
      DescriptorFields fields;
      if (!parse_volume_descriptor(bytes, sector_count, fields)) {
        return VfsError::invalid_source;
      }
      if (!found_primary) {
        fields.label = decode_label(bytes, false);
        primary = std::move(fields);
        found_primary = true;
      }
    } else if (type == kTypeSupplementary && has_joliet_escape(bytes)) {
      // Supplementary descriptors without a Joliet escape sequence describe an
      // encoding this reader does not interpret. They are skipped rather than
      // validated, so a defect in one never rejects the primary tree.
      DescriptorFields fields;
      if (!parse_volume_descriptor(bytes, sector_count, fields)) {
        return VfsError::invalid_source;
      }
      if (!joliet.joliet) {
        fields.joliet = true;
        fields.label = decode_label(bytes, true);
        joliet = std::move(fields);
      }
    }
  }

  if (!found_primary || !found_terminator) {
    return VfsError::invalid_source;
  }

  const auto& selected = joliet.joliet ? joliet : primary;
  state.volume_set_size = primary.volume_set_size;
  state.volume_blocks = selected.volume_blocks;
  state.root_extent = selected.root_extent;
  state.root_size = selected.root_size;
  state.joliet = joliet.joliet;
  state.label = selected.label.empty() ? primary.label : selected.label;
  return VfsError::none;
}

// --- Directory records -----------------------------------------------------

[[nodiscard]] bool strip_version_suffix(std::string& name) noexcept {
  const auto separator = name.rfind(';');
  if (separator != std::string::npos) {
    const auto digits = name.size() - separator - 1U;
    if (separator == 0 || digits == 0 || digits > 5) {
      return false;
    }
    for (auto index = separator + 1U; index < name.size(); ++index) {
      if (name[index] < '0' || name[index] > '9') {
        return false;
      }
    }
    name.erase(separator);
  }
  if (name.size() > 1 && name.back() == '.') {
    name.pop_back();
  }
  return !name.empty();
}

[[nodiscard]] bool decode_primary_name(
    const std::span<const std::byte> identifier, std::string& name) {
  if (identifier.size() > Iso9660Limits::hard_max_decoded_name_bytes) {
    return false;
  }
  name.clear();
  name.reserve(identifier.size());
  for (const auto value : identifier) {
    const auto byte = std::to_integer<std::uint8_t>(value);
    if (byte < 0x20U || byte > 0x7eU || byte == '/' || byte == '\\') {
      return false;
    }
    name.push_back(static_cast<char>(byte));
  }
  return strip_version_suffix(name);
}

// Joliet names are UCS-2 in big-endian order. Surrogate code units are not
// representable in UCS-2 and are rejected rather than repaired.
[[nodiscard]] bool decode_joliet_name(
    const std::span<const std::byte> identifier, std::string& name) {
  if ((identifier.size() % 2U) != 0 ||
      identifier.size() > Iso9660Limits::hard_max_decoded_name_bytes) {
    return false;
  }
  name.clear();
  name.reserve(identifier.size());
  for (std::size_t index = 0; index + 1 < identifier.size(); index += 2) {
    const auto unit = static_cast<std::uint32_t>(
        (static_cast<std::uint32_t>(byte_at(identifier, index)) << 8U) |
        static_cast<std::uint32_t>(byte_at(identifier, index + 1)));
    if (unit < 0x20U || unit == '/' || unit == '\\' ||
        (unit >= 0xd800U && unit <= 0xdfffU)) {
      return false;
    }
    if (unit < 0x80U) {
      name.push_back(static_cast<char>(unit));
    } else if (unit < 0x800U) {
      name.push_back(static_cast<char>(0xc0U | (unit >> 6U)));
      name.push_back(static_cast<char>(0x80U | (unit & 0x3fU)));
    } else {
      name.push_back(static_cast<char>(0xe0U | (unit >> 12U)));
      name.push_back(static_cast<char>(0x80U | ((unit >> 6U) & 0x3fU)));
      name.push_back(static_cast<char>(0x80U | (unit & 0x3fU)));
    }
  }
  return strip_version_suffix(name);
}

struct IsoRecord {
  std::string name;
  std::uint32_t extent{0};
  std::uint32_t size{0};
  bool directory{false};
};

enum class RecordStatus {
  entry,
  skip,
  end_of_sector,
  invalid,
};

[[nodiscard]] RecordStatus parse_directory_record(
    const std::span<const std::byte> sector, const std::size_t offset,
    const IsoState& state, IsoRecord& record, std::size_t& consumed) {
  if (offset >= sector.size()) {
    return RecordStatus::end_of_sector;
  }
  const auto length = byte_at(sector, offset);
  if (length == 0) {
    return RecordStatus::end_of_sector;
  }
  if (length < kDirectoryRecordBytes ||
      static_cast<std::size_t>(length) > sector.size() - offset) {
    return RecordStatus::invalid;
  }
  consumed = length;

  const auto bytes = sector.subspan(offset, length);
  // An extended attribute record would displace the file data; the project
  // reader does not interpret one and refuses to guess its length.
  if (byte_at(bytes, 1) != 0) {
    return RecordStatus::invalid;
  }

  std::uint32_t extent = 0;
  std::uint32_t size = 0;
  std::uint16_t volume_sequence = 0;
  if (!read_both_endian_u32(bytes, 2, extent) ||
      !read_both_endian_u32(bytes, 10, size) ||
      !read_both_endian_u16(bytes, 28, volume_sequence)) {
    return RecordStatus::invalid;
  }
  // Only the first volume of a volume set is readable here, so a record that
  // claims to live on another volume is rejected rather than mis-read.
  if (volume_sequence != 1 || volume_sequence > state.volume_set_size) {
    return RecordStatus::invalid;
  }

  const auto flags = byte_at(bytes, 25);
  if ((flags & kFlagMultiExtent) != 0) {
    // Multi-extent files are not supported; accepting only the first extent
    // would silently truncate the file.
    return RecordStatus::invalid;
  }
  if (byte_at(bytes, 26) != 0 || byte_at(bytes, 27) != 0) {
    // Interleaved file layouts are not supported.
    return RecordStatus::invalid;
  }

  const auto identifier_length = byte_at(bytes, 32);
  if (static_cast<std::size_t>(identifier_length) >
      bytes.size() - kFileIdentifierOffset) {
    return RecordStatus::invalid;
  }
  const auto identifier =
      bytes.subspan(kFileIdentifierOffset, identifier_length);

  const bool directory = (flags & kFlagDirectory) != 0;
  const auto extent_sectors = directory ? std::max<std::uint64_t>(
                                              sectors_for(size), 1U)
                                        : sectors_for(size);
  if (static_cast<std::uint64_t>(extent) + extent_sectors >
      state.volume_blocks) {
    return RecordStatus::invalid;
  }
  // ECMA-119 9.1.4: a directory's data length is a whole number of blocks.
  if (directory &&
      (size == 0 || (size % kBlockSize) != 0 ||
       sectors_for(size) > Iso9660Limits::hard_max_directory_sectors)) {
    return RecordStatus::invalid;
  }

  if (identifier_length == 1) {
    const auto marker = byte_at(identifier, 0);
    if (marker == 0x00U || marker == 0x01U) {
      // "." and ".." are structural entries and are never surfaced.
      return RecordStatus::skip;
    }
  }
  if (identifier_length == 0) {
    return RecordStatus::invalid;
  }
  if ((flags & kFlagAssociated) != 0) {
    return RecordStatus::skip;
  }

  std::string name;
  const bool decoded = state.joliet ? decode_joliet_name(identifier, name)
                                    : decode_primary_name(identifier, name);
  if (!decoded ||
      static_cast<std::uint64_t>(name.size()) >
          state.limits.directory.max_page_name_bytes) {
    return RecordStatus::invalid;
  }

  record.name = std::move(name);
  record.extent = extent;
  record.size = size;
  record.directory = directory;
  return RecordStatus::entry;
}

enum class ScanStatus {
  entry,
  end,
  error,
  limit,
};

// Walks one directory extent sector by sector. ECMA-119 forbids a directory
// record from spanning a logical sector, so a zero length byte simply ends
// the current sector.
class IsoDirectoryScanner final {
 public:
  IsoDirectoryScanner(std::shared_ptr<IsoState> owner,
                      const std::uint32_t extent, const std::uint32_t size)
      : owner_{std::move(owner)},
        extent_{extent},
        sectors_{static_cast<std::uint32_t>(std::min<std::uint64_t>(
            sectors_for(size), Iso9660Limits::hard_max_directory_sectors))} {}

  [[nodiscard]] ScanStatus next(IsoRecord& record) {
    while (true) {
      if (examined_ >= Iso9660Limits::hard_max_records_examined) {
        return ScanStatus::limit;
      }
      ++examined_;
      if (!loaded_) {
        if (sector_index_ >= sectors_) {
          return ScanStatus::end;
        }
        const auto error =
            read_block(owner_->source,
                       static_cast<std::uint64_t>(extent_) + sector_index_,
                       block_);
        if (error != VfsError::none) {
          return ScanStatus::error;
        }
        loaded_ = true;
        offset_ = 0;
      }

      std::size_t consumed = 0;
      const auto status = parse_directory_record(
          std::span<const std::byte>{block_}, offset_, *owner_, record,
          consumed);
      switch (status) {
        case RecordStatus::end_of_sector:
          loaded_ = false;
          ++sector_index_;
          continue;
        case RecordStatus::invalid:
          return ScanStatus::error;
        case RecordStatus::skip:
          offset_ += consumed;
          continue;
        case RecordStatus::entry:
          offset_ += consumed;
          return ScanStatus::entry;
      }
      return ScanStatus::error;
    }
  }

 private:
  std::shared_ptr<IsoState> owner_;
  std::uint32_t extent_{0};
  std::uint32_t sectors_{0};
  std::uint32_t sector_index_{0};
  std::size_t offset_{0};
  std::uint64_t examined_{0};
  bool loaded_{false};
  Block block_{};
};

class IsoDirectoryProvider final : public detail::DirectoryEntryProvider {
 public:
  IsoDirectoryProvider(std::shared_ptr<IsoState> owner,
                       const std::uint32_t extent, const std::uint32_t size)
      : owner_{owner}, scanner_{std::move(owner), extent, size} {}

  [[nodiscard]] detail::DirectoryProviderResult next() override {
    const std::scoped_lock lock{owner_->mutex};
    IsoRecord record;
    switch (scanner_.next(record)) {
      case ScanStatus::end:
        return {.error = VfsError::none, .end = true, .entry = {}};
      case ScanStatus::error:
        return {.error = VfsError::read_failed, .end = false, .entry = {}};
      case ScanStatus::limit:
        return {.error = VfsError::limit_exceeded, .end = false, .entry = {}};
      case ScanStatus::entry:
        break;
    }
    return {
        .error = VfsError::none,
        .end = false,
        .entry =
            DirectoryEntry{
                .name = std::move(record.name),
                .type = record.directory ? EntryType::directory
                                         : EntryType::file,
                .size_bytes = record.directory
                                  ? 0U
                                  : static_cast<std::uint64_t>(record.size),
            },
    };
  }

 private:
  std::shared_ptr<IsoState> owner_;
  IsoDirectoryScanner scanner_;
};

// Primary-tree identifiers are recorded in upper case, so lookups fold ASCII
// case. Joliet identifiers preserve case and are compared exactly.
[[nodiscard]] bool names_match(const std::string_view left,
                               const std::string_view right,
                               const bool fold_case) noexcept {
  if (left.size() != right.size()) {
    return false;
  }
  if (!fold_case) {
    return left == right;
  }
  const auto fold = [](const char value) noexcept {
    const auto byte = static_cast<unsigned char>(value);
    return byte >= 'A' && byte <= 'Z'
               ? static_cast<unsigned char>(byte - 'A' + 'a')
               : byte;
  };
  for (std::size_t index = 0; index < left.size(); ++index) {
    if (fold(left[index]) != fold(right[index])) {
      return false;
    }
  }
  return true;
}

struct Located {
  VfsError error{VfsError::none};
  std::uint32_t extent{0};
  std::uint32_t size{0};
  bool directory{false};
};

// Resolves a normalized path from the root extent. Cycle detection is by
// extent: a child directory may never reuse an extent already on the path.
[[nodiscard]] Located locate(const std::shared_ptr<IsoState>& state,
                             const std::string_view normalized_path) {
  Located result{
      .error = VfsError::none,
      .extent = state->root_extent,
      .size = state->root_size,
      .directory = true,
  };
  std::vector<std::uint32_t> visited;
  visited.reserve(state->limits.directory.max_path_components);
  visited.push_back(state->root_extent);

  std::size_t position = 0;
  while (position < normalized_path.size()) {
    while (position < normalized_path.size() &&
           normalized_path[position] == '/') {
      ++position;
    }
    if (position >= normalized_path.size()) {
      break;
    }
    auto end = normalized_path.find('/', position);
    if (end == std::string_view::npos) {
      end = normalized_path.size();
    }
    const auto component = normalized_path.substr(position, end - position);
    position = end;

    if (!result.directory) {
      return {.error = VfsError::not_found};
    }
    if (visited.size() >= state->limits.directory.max_path_components) {
      return {.error = VfsError::limit_exceeded};
    }

    IsoDirectoryScanner scanner{state, result.extent, result.size};
    IsoRecord record;
    bool found = false;
    while (!found) {
      const auto status = scanner.next(record);
      if (status == ScanStatus::end) {
        return {.error = VfsError::not_found};
      }
      if (status == ScanStatus::error) {
        return {.error = VfsError::read_failed};
      }
      if (status == ScanStatus::limit) {
        return {.error = VfsError::limit_exceeded};
      }
      if (names_match(record.name, component, !state->joliet)) {
        found = true;
      }
    }

    if (record.directory &&
        std::find(visited.begin(), visited.end(), record.extent) !=
            visited.end()) {
      return {.error = VfsError::invalid_source};
    }
    visited.push_back(record.extent);
    result.extent = record.extent;
    result.size = record.size;
    result.directory = record.directory;
  }
  return result;
}

}  // namespace

// --- Iso9660File -----------------------------------------------------------

struct Iso9660File::Impl {
  std::shared_ptr<IsoState> owner;
  std::uint32_t extent{0};
  std::uint64_t file_size{0};
  std::uint64_t position{0};
};

Iso9660File::Iso9660File(std::unique_ptr<Impl> implementation) noexcept
    : implementation_{std::move(implementation)} {}

Iso9660File::~Iso9660File() = default;
Iso9660File::Iso9660File(Iso9660File&&) noexcept = default;
Iso9660File& Iso9660File::operator=(Iso9660File&&) noexcept = default;

std::uint64_t Iso9660File::size() const noexcept {
  return implementation_ == nullptr ? 0 : implementation_->file_size;
}

std::int64_t Iso9660File::tell() const noexcept {
  if (implementation_ == nullptr) {
    return -1;
  }
  const std::scoped_lock lock{implementation_->owner->mutex};
  return static_cast<std::int64_t>(implementation_->position);
}

std::int64_t Iso9660File::read(const std::span<std::byte> destination) {
  if (implementation_ == nullptr || destination.empty()) {
    return 0;
  }
  const std::scoped_lock lock{implementation_->owner->mutex};
  const auto remaining = implementation_->file_size - implementation_->position;
  if (remaining == 0) {
    return 0;
  }
  const auto count = static_cast<std::size_t>(std::min<std::uint64_t>(
      remaining,
      std::min<std::uint64_t>(
          destination.size(),
          static_cast<std::uint64_t>(
              std::numeric_limits<std::int64_t>::max()))));
  const auto offset =
      (static_cast<std::uint64_t>(implementation_->extent) * kBlockSize) +
      implementation_->position;
  if (implementation_->owner->source == nullptr ||
      implementation_->owner->source->read_exact_at(
          offset, destination.first(count)) !=
          platform::MediaSourceError::none) {
    return -1;
  }
  implementation_->position += count;
  return static_cast<std::int64_t>(count);
}

bool Iso9660File::seek(const std::uint64_t offset) {
  if (implementation_ == nullptr || offset > implementation_->file_size ||
      offset > static_cast<std::uint64_t>(
                   std::numeric_limits<std::int64_t>::max())) {
    return false;
  }
  const std::scoped_lock lock{implementation_->owner->mutex};
  implementation_->position = offset;
  return true;
}

// --- Iso9660Archive --------------------------------------------------------

struct Iso9660Archive::Impl {
  std::shared_ptr<IsoState> state;
};

Iso9660Archive::Iso9660Archive() : implementation_{std::make_unique<Impl>()} {}
Iso9660Archive::~Iso9660Archive() = default;
Iso9660Archive::Iso9660Archive(Iso9660Archive&&) noexcept = default;
Iso9660Archive& Iso9660Archive::operator=(Iso9660Archive&&) noexcept = default;

VfsError Iso9660Archive::open(SharedMediaSource source,
                              const UdfArchiveLimits limits) {
  if (implementation_ == nullptr) {
    implementation_ = std::make_unique<Impl>();
  }
  close();
  if (!detail::valid_directory_limits(limits)) {
    return VfsError::limit_exceeded;
  }
  if (source == nullptr) {
    return VfsError::invalid_source;
  }

  const auto source_size = source->size();
  if (source_size == 0 || source_size > limits.max_source_bytes ||
      source_size > UdfArchiveLimits::max_representable_source_bytes) {
    return VfsError::limit_exceeded;
  }
  const auto sector_count = source_size / kBlockSize;
  if (sector_count <= kFirstDescriptorSector) {
    return VfsError::invalid_source;
  }

  const auto initial = source->verify_unchanged();
  if (initial != platform::MediaSourceError::none) {
    return map_source_error(initial);
  }

  auto state = std::make_shared<IsoState>(source, limits);
  const auto mounted = mount_descriptors(source, sector_count, *state);
  if (mounted != VfsError::none) {
    return mounted;
  }

  const auto stable = source->verify_unchanged();
  if (stable != platform::MediaSourceError::none) {
    return map_source_error(stable);
  }
  implementation_->state = std::move(state);
  return VfsError::none;
}

Iso9660Archive Iso9660Archive::share() const {
  Iso9660Archive result;
  if (implementation_ != nullptr) {
    result.implementation_->state = implementation_->state;
  }
  return result;
}

void Iso9660Archive::close() noexcept {
  if (implementation_ != nullptr) {
    implementation_->state.reset();
  }
}

bool Iso9660Archive::is_open() const noexcept {
  return implementation_ != nullptr && implementation_->state != nullptr;
}

bool Iso9660Archive::uses_joliet() const noexcept {
  return is_open() && implementation_->state->joliet;
}

std::string Iso9660Archive::volume_label() const {
  if (!is_open()) {
    return {};
  }
  const std::scoped_lock lock{implementation_->state->mutex};
  return implementation_->state->label;
}

DirectoryPage Iso9660Archive::list_page(const std::string_view path) const {
  DirectoryPage result;
  if (!is_open()) {
    result.error = VfsError::not_open;
    return result;
  }

  const auto state = implementation_->state;
  const auto normalized = normalize_path(path);
  if (!normalized.has_value()) {
    result.error = VfsError::invalid_path;
    return result;
  }
  if (!detail::path_within_depth(*normalized, state->limits)) {
    result.error = VfsError::limit_exceeded;
    return result;
  }

  const std::scoped_lock lock{state->mutex};
  const auto initial = state->source->verify_unchanged();
  if (initial != platform::MediaSourceError::none) {
    result.error = map_source_error(initial);
    return result;
  }

  const auto located = locate(state, *normalized);
  if (located.error != VfsError::none) {
    result.error = located.error;
    return result;
  }
  if (!located.directory) {
    result.error = VfsError::not_found;
    return result;
  }

  auto engine = std::make_unique<detail::DirectoryPageEngine>(
      std::make_unique<IsoDirectoryProvider>(state, located.extent,
                                             located.size),
      state->limits);
  auto page = engine->next_page();
  const auto final_state = state->source->verify_unchanged();
  if (final_state != platform::MediaSourceError::none) {
    result.error = map_source_error(final_state);
    return result;
  }
  if (page.error != VfsError::none) {
    result.error = page.error;
    return result;
  }

  result.entries = std::move(page.entries);
  if (page.has_more) {
    auto cursor = std::make_unique<DirectoryCursor::Impl>();
    cursor->owner = state;
    cursor->engine = std::move(engine);
    result.cursor = DirectoryCursor{std::move(cursor)};
  }
  return result;
}

DirectoryPage Iso9660Archive::continue_list(DirectoryCursor cursor) const {
  DirectoryPage result;
  if (!cursor.valid() || !is_open() ||
      cursor.implementation_->owner.get() !=
          static_cast<void*>(implementation_->state.get())) {
    result.error = VfsError::invalid_cursor;
    return result;
  }

  const auto state =
      std::static_pointer_cast<IsoState>(cursor.implementation_->owner);
  auto engine = std::move(cursor.implementation_->engine);
  cursor.implementation_.reset();
  const std::scoped_lock lock{state->mutex};
  const auto initial = state->source->verify_unchanged();
  if (initial != platform::MediaSourceError::none) {
    result.error = map_source_error(initial);
    return result;
  }
  auto page = engine->next_page();
  const auto final_state = state->source->verify_unchanged();
  if (final_state != platform::MediaSourceError::none) {
    result.error = map_source_error(final_state);
    return result;
  }
  if (page.error != VfsError::none) {
    result.error = page.error;
    return result;
  }

  result.entries = std::move(page.entries);
  if (page.has_more) {
    auto next_cursor = std::make_unique<DirectoryCursor::Impl>();
    next_cursor->owner = state;
    next_cursor->engine = std::move(engine);
    result.cursor = DirectoryCursor{std::move(next_cursor)};
  }
  return result;
}

DirectoryListing Iso9660Archive::list(const std::string_view path) const {
  DirectoryListing result;
  auto page = list_page(path);
  while (true) {
    if (page.error != VfsError::none) {
      result.error = page.error;
      result.entries.clear();
      return result;
    }
    result.entries.insert(result.entries.end(),
                          std::make_move_iterator(page.entries.begin()),
                          std::make_move_iterator(page.entries.end()));
    if (page.complete()) {
      return result;
    }
    page = continue_list(std::move(page.cursor));
  }
}

std::unique_ptr<Iso9660File> Iso9660Archive::open_file(
    const std::string_view path) const {
  if (!is_open()) {
    return nullptr;
  }
  const auto normalized = normalize_path(path);
  if (!normalized.has_value() || *normalized == "/") {
    return nullptr;
  }
  const auto state = implementation_->state;
  if (!detail::path_within_depth(*normalized, state->limits)) {
    return nullptr;
  }

  const std::scoped_lock lock{state->mutex};
  if (state->source->verify_unchanged() != platform::MediaSourceError::none) {
    return nullptr;
  }
  const auto located = locate(state, *normalized);
  if (located.error != VfsError::none || located.directory) {
    return nullptr;
  }

  auto file_implementation = std::make_unique<Iso9660File::Impl>();
  file_implementation->owner = state;
  file_implementation->extent = located.extent;
  file_implementation->file_size = static_cast<std::uint64_t>(located.size);
  return std::unique_ptr<Iso9660File>{
      new Iso9660File{std::move(file_implementation)}};
}

std::unique_ptr<Iso9660File> Iso9660Archive::open_file_at(
    const std::string_view directory_path,
    const std::string_view entry_name) const {
  if (!is_open() || !is_single_path_component(entry_name)) {
    return nullptr;
  }
  const auto directory = normalize_path(directory_path);
  if (!directory.has_value()) {
    return nullptr;
  }
  std::string combined{*directory};
  if (combined.back() != '/') {
    combined.push_back('/');
  }
  combined.append(entry_name);
  return open_file(combined);
}

}  // namespace ohl::vfs
