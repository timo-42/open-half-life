#pragma once

#include <cstddef>
#include <cstdint>
#include <memory>
#include <string>
#include <string_view>
#include <vector>

namespace ohl::vfs {
class UdfArchive;
}

namespace ohl::media {

enum class CabinetError {
  none,
  source_not_open,
  invalid_path,
  invalid_entry_path,
  too_many_entries,
  unsupported_or_corrupt,
};

struct CabinetEntry {
  int index{-1};
  // Validated lexical path; extraction still requires native no-follow writes.
  std::string safe_relative_path;
  std::uint64_t size_bytes{0};
};

struct CabinetListing {
  // WARNING: entries may contain partial diagnostic output when error is not
  // none. Importers must require valid(); rejected entries never expose names.
  CabinetError error{CabinetError::none};
  std::size_t total_entries{0};
  std::size_t rejected_entries{0};
  std::vector<CabinetEntry> entries;

  [[nodiscard]] bool valid() const noexcept {
    return error == CabinetError::none;
  }
};

class InstallShieldCabinet final {
 public:
  InstallShieldCabinet();
  ~InstallShieldCabinet();
  InstallShieldCabinet(InstallShieldCabinet&&) noexcept;
  InstallShieldCabinet& operator=(InstallShieldCabinet&&) noexcept;

  InstallShieldCabinet(const InstallShieldCabinet&) = delete;
  InstallShieldCabinet& operator=(const InstallShieldCabinet&) = delete;

  [[nodiscard]] CabinetError open(const ohl::vfs::UdfArchive& source,
                                  std::string_view cabinet_path);
  void close() noexcept;
  [[nodiscard]] bool is_open() const noexcept;
  [[nodiscard]] CabinetListing entries(
      std::size_t maximum_entries = 50'000) const;

 private:
  struct Impl;
  std::unique_ptr<Impl> implementation_;
};

}  // namespace ohl::media
