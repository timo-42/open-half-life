#include "ohl/media/installshield_cabinet.hpp"
#include "ohl/media/payload_layout.hpp"
#include "ohl/media/payload_path.hpp"
#include "ohl/vfs/udf_archive.hpp"

#include <cstdint>
#include <filesystem>
#include <iostream>
#include <vector>

namespace {

constexpr const char* kSyntheticCabinetPath =
    "ohl-synthetic-fixture/sentinel.cab";
constexpr const char* kSyntheticInvalidCabinetPath =
    "../ohl-synthetic-fixture/sentinel.cab";

}  // namespace

int main(const int argument_count, const char* const arguments[]) {
  ohl::vfs::UdfArchive source;
  ohl::media::InstallShieldCabinet cabinet;
  if (cabinet.open(source, kSyntheticCabinetPath) !=
      ohl::media::CabinetError::source_not_open) {
    std::cerr << "closed source was not rejected\n";
    return 1;
  }
  if (cabinet.entries().error != ohl::media::CabinetError::source_not_open) {
    std::cerr << "closed cabinet listing returned the wrong error\n";
    return 1;
  }

  if (argument_count != 1 && argument_count != 3) {
    std::cerr << "owned-media integration requires both runtime paths\n";
    return 2;
  }
  if (argument_count == 1) {
    return 0;
  }

  if (source.open(std::filesystem::path{arguments[1]}) !=
      ohl::vfs::VfsError::none) {
    std::cerr << "runtime media could not be opened\n";
    return 1;
  }
  if (cabinet.open(source, kSyntheticInvalidCabinetPath) !=
      ohl::media::CabinetError::invalid_path) {
    std::cerr << "synthetic invalid cabinet path was not rejected\n";
    return 1;
  }
  cabinet.close();
  if (cabinet.is_open()) {
    std::cerr << "failed cabinet remained open after close\n";
    return 1;
  }
  if (cabinet.open(source, arguments[2]) != ohl::media::CabinetError::none) {
    std::cerr << "runtime cabinet could not be opened\n";
    return 1;
  }
  if (cabinet.entries(0).error !=
      ohl::media::CabinetError::too_many_entries) {
    std::cerr << "entry bound was not enforced\n";
    return 1;
  }
  source.close();
  const auto listing = cabinet.entries();
  if (listing.entries.empty() ||
      (listing.error != ohl::media::CabinetError::none &&
       listing.error != ohl::media::CabinetError::unsupported_or_corrupt) ||
      listing.entries.size() + listing.rejected_entries !=
          listing.total_entries) {
    std::cerr << "cabinet aggregate validation failed with error "
              << static_cast<int>(listing.error) << '\n';
    return 1;
  }
  const auto& entries = listing.entries;
  std::vector<ohl::media::PayloadEntryMetadata> metadata;
  metadata.reserve(entries.size());
  for (const auto& entry : entries) {
    if (entry.index < 0 ||
        !ohl::media::validate_payload_path(entry.safe_relative_path).valid()) {
      std::cerr << "cabinet contained a rejected path\n";
      return 1;
    }
    metadata.push_back({static_cast<std::uint64_t>(entry.index),
                        entry.safe_relative_path, entry.size_bytes});
  }

  if (listing.valid()) {
    const auto layout = ohl::media::plan_payload_layout(metadata);
    switch (layout.error) {
      case ohl::media::PayloadLayoutError::none:
        if (layout.entries.size() != metadata.size() ||
            layout.rejected_entry.has_value() ||
            layout.path_error != ohl::media::PayloadPathError::none) {
          std::cerr << "successful layout result was inconsistent\n";
          return 1;
        }
        break;
      case ohl::media::PayloadLayoutError::too_many_path_bytes:
      case ohl::media::PayloadLayoutError::path_conflict:
      case ohl::media::PayloadLayoutError::entry_too_large:
      case ohl::media::PayloadLayoutError::payload_too_large:
        if (!layout.rejected_entry.has_value() ||
            *layout.rejected_entry >= metadata.size() ||
            !layout.entries.empty()) {
          std::cerr << "rejected layout result was inconsistent\n";
          return 1;
        }
        break;
      case ohl::media::PayloadLayoutError::invalid_limits:
      case ohl::media::PayloadLayoutError::too_many_entries:
      case ohl::media::PayloadLayoutError::source_token_conflict:
      case ohl::media::PayloadLayoutError::invalid_path:
        std::cerr << "validated adapter metadata violated layout preconditions\n";
        return 1;
    }
  }
  return 0;
}
