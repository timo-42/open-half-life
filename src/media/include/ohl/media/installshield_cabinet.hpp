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
  unsupported_or_corrupt,
};

struct CabinetEntry {
  int index{-1};
  // Empty when the archive-provided path is unsafe for extraction.
  std::string safe_relative_path;
  std::uint64_t size_bytes{0};
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
  [[nodiscard]] std::vector<CabinetEntry> entries() const;

 private:
  struct Impl;
  std::unique_ptr<Impl> implementation_;
};

}  // namespace ohl::media
