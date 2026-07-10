#include "ohl/media/installshield_cabinet.hpp"
#include "ohl/media/payload_layout.hpp"
#include "ohl/media/payload_path.hpp"

#include "ohl/vfs/udf_archive.hpp"

#include <filesystem>
#include <iostream>
#include <vector>

int main(const int argument_count, const char* const arguments[]) {
  ohl::vfs::UdfArchive source;
  ohl::media::InstallShieldCabinet cabinet;
  if (cabinet.open(source, "/data1.cab") !=
      ohl::media::CabinetError::source_not_open) {
    std::cerr << "closed VFS returned the wrong cabinet error\n";
    return 1;
  }
  if (cabinet.entries().error != ohl::media::CabinetError::source_not_open) {
    std::cerr << "closed cabinet listing returned the wrong error\n";
    return 1;
  }

  if (argument_count == 2) {
    if (source.open(std::filesystem::path{arguments[1]}) !=
        ohl::vfs::VfsError::none) {
      std::cerr << "integration image did not mount\n";
      return 1;
    }
    if (cabinet.open(source, "/missing-cabinet.cab") !=
        ohl::media::CabinetError::unsupported_or_corrupt) {
      std::cerr << "missing cabinet returned the wrong error\n";
      return 1;
    }
    cabinet.close();
    if (cabinet.is_open()) {
      std::cerr << "failed cabinet remained open after close\n";
      return 1;
    }
    if (cabinet.open(source, "/data1.cab") !=
        ohl::media::CabinetError::none) {
      std::cerr << "integration cabinet did not open\n";
      return 1;
    }
    if (cabinet.entries(0).error !=
        ohl::media::CabinetError::too_many_entries) {
      std::cerr << "cabinet entry bound was not enforced\n";
      return 1;
    }
    source.close();
    const auto listing = cabinet.entries();
    if (listing.entries.empty() ||
        (listing.error != ohl::media::CabinetError::none &&
         listing.error != ohl::media::CabinetError::unsupported_or_corrupt) ||
        listing.entries.size() + listing.rejected_entries !=
            listing.total_entries) {
      std::cerr << "integration cabinet did not contain entries: "
                << static_cast<int>(listing.error) << '\n';
      return 1;
    }
    const auto& entries = listing.entries;
    std::vector<ohl::media::PayloadEntryMetadata> metadata;
    metadata.reserve(entries.size());
    for (const auto& entry : entries) {
      if (!ohl::media::validate_payload_path(entry.safe_relative_path).valid()) {
        std::cerr << "integration cabinet contained a non-portable path\n";
        return 1;
      }
      metadata.push_back({static_cast<std::uint64_t>(entry.index),
                          entry.safe_relative_path, entry.size_bytes});
    }
    const auto layout = ohl::media::plan_payload_layout(metadata);
    // Installers may contain alternative component entries targeting the same
    // destination. Import must select components before final layout planning.
    if (!layout.valid() &&
        layout.error != ohl::media::PayloadLayoutError::path_conflict) {
      std::cerr << "integration cabinet did not have a portable layout: "
                << static_cast<int>(layout.error) << " at entry "
                << layout.rejected_entry.value_or(entries.size()) << '\n';
      return 1;
    }
  }
  return 0;
}
