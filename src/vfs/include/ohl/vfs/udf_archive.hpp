#pragma once

#include <cstddef>
#include <cstdint>
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

namespace detail {
class DirectoryArchiveTestHarness;
}

enum class VfsError {
  none,
  not_open,
  invalid_path,
  invalid_cursor,
  invalid_source,
  limit_exceeded,
  source_changed,
  open_failed,
  not_found,
  read_failed,
};

struct UdfDirectoryLimits {
  // Logical result accounting is platform-independent: the encoded name plus
  // one type byte and one 64-bit size value for each returned entry.
  static constexpr std::uint64_t logical_entry_metadata_bytes =
      sizeof(std::uint8_t) + sizeof(std::uint64_t);
  static constexpr std::uint32_t hard_max_path_components = 64;
  static constexpr std::uint32_t hard_max_page_entries = 256;
  static constexpr std::uint64_t hard_max_page_name_bytes = 64ULL * 1'024ULL;
  static constexpr std::uint64_t hard_max_page_result_bytes = 96ULL * 1'024ULL;
  static constexpr std::uint32_t hard_max_page_work = 1'024;
  static constexpr std::uint32_t hard_max_page_count = 64;
  static constexpr std::uint64_t hard_max_cursor_work = 65'536;

  std::uint32_t max_path_components{hard_max_path_components};
  std::uint32_t max_page_entries{hard_max_page_entries};
  std::uint64_t max_page_name_bytes{hard_max_page_name_bytes};
  std::uint64_t max_page_result_bytes{hard_max_page_result_bytes};
  std::uint32_t max_page_work{hard_max_page_work};
  std::uint32_t max_page_count{hard_max_page_count};
  std::uint64_t max_cursor_work{hard_max_cursor_work};
};

struct UdfArchiveLimits {
  static constexpr std::uint64_t logical_block_size = 2'048;
  static constexpr std::uint64_t max_representable_source_bytes =
      static_cast<std::uint64_t>(std::numeric_limits<std::uint32_t>::max()) *
      logical_block_size;

  std::uint64_t max_source_bytes{max_representable_source_bytes};
  std::uint32_t max_blocks_per_read{
      static_cast<std::uint32_t>(std::numeric_limits<std::int32_t>::max())};
  // Callers may lower directory ceilings before open(). A successful mount
  // copies them into immutable archive/cursor state. Zero or raised values are
  // rejected rather than removing a bound.
  UdfDirectoryLimits directory;
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

class DirectoryCursor final {
 public:
  DirectoryCursor();
  ~DirectoryCursor();
  DirectoryCursor(DirectoryCursor&&) noexcept;
  DirectoryCursor& operator=(DirectoryCursor&&) noexcept;

  DirectoryCursor(const DirectoryCursor&) = delete;
  DirectoryCursor& operator=(const DirectoryCursor&) = delete;

  [[nodiscard]] bool valid() const noexcept;

 private:
  struct Impl;
  explicit DirectoryCursor(std::unique_ptr<Impl> implementation) noexcept;
  std::unique_ptr<Impl> implementation_;

  friend class UdfArchive;
};

class DirectoryPage final {
 public:
  DirectoryPage();
  ~DirectoryPage();
  DirectoryPage(DirectoryPage&&) noexcept;
  DirectoryPage& operator=(DirectoryPage&&) noexcept;

  DirectoryPage(const DirectoryPage&) = delete;
  DirectoryPage& operator=(const DirectoryPage&) = delete;

  VfsError error{VfsError::none};
  std::vector<DirectoryEntry> entries;
  DirectoryCursor cursor;

  [[nodiscard]] bool complete() const noexcept {
    return error == VfsError::none && !cursor.valid();
  }
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
  // Returns another read-only handle that keeps the mounted archive alive.
  [[nodiscard]] UdfArchive share() const;
  void close() noexcept;
  [[nodiscard]] bool is_open() const noexcept;
  [[nodiscard]] std::string volume_label() const;
  // Returns one bounded page in deterministic on-media order. A valid cursor
  // is present only when another page is available.
  [[nodiscard]] DirectoryPage list_page(std::string_view path) const;
  // Consumes a cursor produced by this archive or one of its same-state shares.
  [[nodiscard]] DirectoryPage continue_list(DirectoryCursor cursor) const;
  // Compatibility API: succeeds only with the complete bounded result. Any
  // enumeration limit returns limit_exceeded with an empty result.
  [[nodiscard]] DirectoryListing list(std::string_view path) const;
  [[nodiscard]] std::unique_ptr<UdfFile> open_file(
      std::string_view path) const;
  [[nodiscard]] std::unique_ptr<UdfFile> open_file_at(
      std::string_view directory, std::string_view entry_name) const;

 private:
  struct Impl;
  std::unique_ptr<Impl> implementation_;

  friend class detail::DirectoryArchiveTestHarness;
};

}  // namespace ohl::vfs
