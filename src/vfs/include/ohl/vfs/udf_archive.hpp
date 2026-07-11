#pragma once

#include <cstddef>
#include <cstdint>
#include <filesystem>
#include <limits>
#include <memory>
#include <optional>
#include <span>
#include <string>
#include <string_view>
#include <vector>

namespace ohl::platform {
class MediaSource;
}

namespace ohl::vfs {

using SharedMediaSource = std::shared_ptr<const platform::MediaSource>;

enum class VfsError {
  none,
  not_open,
  invalid_path,
  invalid_source,
  limit_exceeded,
  source_changed,
  open_failed,
  not_found,
  read_failed,
};

struct UdfArchiveLimits {
  static constexpr std::uint64_t logical_block_size = 2'048;
  static constexpr std::uint64_t max_representable_source_bytes =
      static_cast<std::uint64_t>(std::numeric_limits<std::uint32_t>::max()) *
      logical_block_size;

  std::uint64_t max_source_bytes{max_representable_source_bytes};
  std::uint32_t max_blocks_per_read{
      static_cast<std::uint32_t>(std::numeric_limits<std::int32_t>::max())};
};

enum class EntryType {
  file,
  directory,
  unknown,
};

struct DirectoryEntry {
  std::string name;
  EntryType type{EntryType::unknown};
  std::uint64_t size_bytes{0};
};

struct DirectoryListing {
  VfsError error{VfsError::none};
  std::vector<DirectoryEntry> entries;
};

[[nodiscard]] std::optional<std::string> normalize_path(
    std::string_view path);
[[nodiscard]] bool is_single_path_component(std::string_view name) noexcept;

class UdfFile final {
 public:
  ~UdfFile();
  UdfFile(UdfFile&&) noexcept;
  UdfFile& operator=(UdfFile&&) noexcept;

  UdfFile(const UdfFile&) = delete;
  UdfFile& operator=(const UdfFile&) = delete;

  [[nodiscard]] std::uint64_t size() const noexcept;
  [[nodiscard]] std::int64_t tell() const noexcept;
  [[nodiscard]] std::int64_t read(std::span<std::byte> destination);
  [[nodiscard]] bool seek(std::uint64_t offset);

 private:
  struct Impl;
  explicit UdfFile(std::unique_ptr<Impl> implementation) noexcept;
  std::unique_ptr<Impl> implementation_;

  friend class UdfArchive;
};

class UdfArchive final {
 public:
  UdfArchive();
  ~UdfArchive();
  UdfArchive(UdfArchive&&) noexcept;
  UdfArchive& operator=(UdfArchive&&) noexcept;

  UdfArchive(const UdfArchive&) = delete;
  UdfArchive& operator=(const UdfArchive&) = delete;

  // Mounts a pinned source capability. The source is retained by libudfread's
  // block adapter for exactly the mounted archive lifetime; no pathname is
  // stored or reopened.
  [[nodiscard]] VfsError open(
      SharedMediaSource source, UdfArchiveLimits limits = {});
  // Transitional compatibility wrapper. Acquires image_path exactly once and
  // delegates immediately to the capability overload.
  [[nodiscard]] VfsError open(const std::filesystem::path& image_path);
  // Returns another read-only handle that keeps the mounted archive alive.
  [[nodiscard]] UdfArchive share() const;
  void close() noexcept;
  [[nodiscard]] bool is_open() const noexcept;
  [[nodiscard]] std::string volume_label() const;
  [[nodiscard]] DirectoryListing list(std::string_view path) const;
  [[nodiscard]] std::unique_ptr<UdfFile> open_file(
      std::string_view path) const;
  [[nodiscard]] std::unique_ptr<UdfFile> open_file_at(
      std::string_view directory, std::string_view entry_name) const;

 private:
  struct Impl;
  std::unique_ptr<Impl> implementation_;
};

}  // namespace ohl::vfs
