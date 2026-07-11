#pragma once

#include "ohl/vfs/udf_archive.hpp"

#include <cstddef>
#include <memory>
#include <string>
#include <string_view>
#include <vector>

namespace ohl::vfs::detail {

struct DirectoryProviderResult {
  VfsError error{VfsError::none};
  bool end{false};
  DirectoryEntry entry;
};

// Source-private provider boundary used by the production libudfread adapter
// and deterministic synthetic page-engine tests. No third-party type crosses
// this interface.
class DirectoryEntryProvider {
 public:
  virtual ~DirectoryEntryProvider() = default;
  [[nodiscard]] virtual DirectoryProviderResult next() = 0;
};

struct DirectoryProviderOpenResult {
  VfsError error{VfsError::none};
  std::unique_ptr<DirectoryEntryProvider> provider;
};

class DirectoryProviderFactory {
 public:
  virtual ~DirectoryProviderFactory() = default;
  [[nodiscard]] virtual DirectoryProviderOpenResult open(
      std::string_view normalized_path) = 0;
};

struct RawDirectoryName {
  const char* data{nullptr};
  std::size_t declared_size{0};
  bool terminated{false};
};

struct BoundedDirectoryNameResult {
  VfsError error{VfsError::none};
  std::string name;

  [[nodiscard]] bool valid() const noexcept {
    return error == VfsError::none;
  }
};

[[nodiscard]] BoundedDirectoryNameResult bounded_directory_name(
    RawDirectoryName input);

struct DirectoryPageEngineResult {
  VfsError error{VfsError::none};
  std::vector<DirectoryEntry> entries;
  bool has_more{false};
};

class DirectoryPageEngine final {
 public:
  DirectoryPageEngine(std::unique_ptr<DirectoryEntryProvider> provider,
                      UdfArchiveLimits limits);
  ~DirectoryPageEngine();
  DirectoryPageEngine(DirectoryPageEngine&&) noexcept;
  DirectoryPageEngine& operator=(DirectoryPageEngine&&) noexcept;

  DirectoryPageEngine(const DirectoryPageEngine&) = delete;
  DirectoryPageEngine& operator=(const DirectoryPageEngine&) = delete;

  [[nodiscard]] DirectoryPageEngineResult next_page();

 private:
  struct Impl;
  std::unique_ptr<Impl> implementation_;
};

// Constructs a source-private synthetic mounted archive that drives the exact
// public list_page/continue_list/list path without libudfread or media. Each
// new list starts from a fresh provider returned by factory.
class DirectoryArchiveTestHarness final {
 public:
  [[nodiscard]] static UdfArchive mount(
      std::shared_ptr<DirectoryProviderFactory> factory,
      UdfArchiveLimits limits = {},
      std::shared_ptr<void> retained_lifetime = {});
};

[[nodiscard]] bool valid_directory_limits(
    const UdfArchiveLimits& limits) noexcept;
[[nodiscard]] bool path_within_depth(std::string_view normalized_path,
                                     const UdfArchiveLimits& limits) noexcept;

}  // namespace ohl::vfs::detail
