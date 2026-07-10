#include "ohl/media/installshield_cabinet.hpp"

#include "ohl/vfs/udf_archive.hpp"

#include <filesystem>
#include <iostream>

int main(const int argument_count, const char* const arguments[]) {
  ohl::vfs::UdfArchive source;
  ohl::media::InstallShieldCabinet cabinet;
  if (cabinet.open(source, "/data1.cab") !=
      ohl::media::CabinetError::source_not_open) {
    std::cerr << "closed VFS returned the wrong cabinet error\n";
    return 1;
  }

  if (argument_count == 2) {
    if (source.open(std::filesystem::path{arguments[1]}) !=
        ohl::vfs::VfsError::none) {
      std::cerr << "integration image did not mount\n";
      return 1;
    }
    if (cabinet.open(source, "/data1.cab") !=
        ohl::media::CabinetError::none) {
      std::cerr << "integration cabinet did not open\n";
      return 1;
    }
    const auto entries = cabinet.entries();
    if (entries.empty()) {
      std::cerr << "integration cabinet did not contain entries\n";
      return 1;
    }
  }
  return 0;
}
