#pragma once

#include "ohl/vfs/udf_archive.hpp"

#include <cstddef>
#include <cstdint>
#include <memory>
#include <span>
#include <string>
#include <string_view>

namespace ohl::vfs {

// Project-owned ECMA-119 (ISO 9660) reader limits. They bound every walk over
// untrusted directory data; nothing here is derived from a recorded field.
struct Iso9660Limits {
  // A directory extent may not exceed this many logical sectors.
  static constexpr std::uint32_t hard_max_directory_sectors = 4'096;
  // Total directory records examined by one enumeration or one path lookup.
  static constexpr std::uint64_t hard_max_records_examined = 262'144;
  // Decoded name bytes accepted from one directory record.
  static constexpr std::size_t hard_max_decoded_name_bytes = 1'024;
};

// A read-only file inside a mounted ECMA-119 volume. The handle keeps the
// mounted archive, and therefore the pinned media source, alive.
class Iso9660File final {
 public:
  ~Iso9660File();
  Iso9660File(Iso9660File&&) noexcept;
  Iso9660File& operator=(Iso9660File&&) noexcept;

  Iso9660File(const Iso9660File&) = delete;
  Iso9660File& operator=(const Iso9660File&) = delete;

  [[nodiscard]] std::uint64_t size() const noexcept;
  [[nodiscard]] std::int64_t tell() const noexcept;
  [[nodiscard]] std::int64_t read(std::span<std::byte> destination);
  [[nodiscard]] bool seek(std::uint64_t offset);

 private:
  struct Impl;
  explicit Iso9660File(std::unique_ptr<Impl> implementation) noexcept;
  std::unique_ptr<Impl> implementation_;

  friend class Iso9660Archive;
};

// Read-only ECMA-119 volume reader with optional Joliet (UCS-2) name support.
// It offers exactly the operations UdfArchive offers and shares the same
// DirectoryEntry, DirectoryPage, DirectoryCursor and limits semantics.
class Iso9660Archive final {
 public:
  Iso9660Archive();
  ~Iso9660Archive();
  Iso9660Archive(Iso9660Archive&&) noexcept;
  Iso9660Archive& operator=(Iso9660Archive&&) noexcept;

  Iso9660Archive(const Iso9660Archive&) = delete;
  Iso9660Archive& operator=(const Iso9660Archive&) = delete;

  // Mounts a pinned source capability. The descriptor set is re-validated
  // here; the preflight result is never trusted as a parsing input. When a
  // valid Joliet supplementary descriptor is present its directory tree is
  // preferred, otherwise the primary tree is used.
  [[nodiscard]] VfsError open(
      SharedMediaSource source, UdfArchiveLimits limits = {});
  // Returns another read-only handle that keeps the mounted archive alive.
  [[nodiscard]] Iso9660Archive share() const;
  void close() noexcept;
  [[nodiscard]] bool is_open() const noexcept;
  [[nodiscard]] bool uses_joliet() const noexcept;
  [[nodiscard]] std::string volume_label() const;
  [[nodiscard]] DirectoryPage list_page(std::string_view path) const;
  [[nodiscard]] DirectoryPage continue_list(DirectoryCursor cursor) const;
  [[nodiscard]] DirectoryListing list(std::string_view path) const;
  [[nodiscard]] std::unique_ptr<Iso9660File> open_file(
      std::string_view path) const;
  [[nodiscard]] std::unique_ptr<Iso9660File> open_file_at(
      std::string_view directory, std::string_view entry_name) const;

 private:
  struct Impl;
  std::unique_ptr<Impl> implementation_;
};

}  // namespace ohl::vfs
