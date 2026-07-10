#include "ohl/media/installshield_cabinet.hpp"
#include "ohl/media/payload_path.hpp"
#include "ohl/vfs/udf_archive.hpp"

#include <filesystem>
#include <iostream>

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
  for (const auto& entry : entries) {
    if (!ohl::media::validate_payload_path(entry.safe_relative_path).valid()) {
      std::cerr << "cabinet contained a rejected path\n";
      return 1;
    }
  }
  return 0;
}
