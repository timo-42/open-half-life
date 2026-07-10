#include "atomic_directory_store_internal.hpp"

#if defined(__linux__)

#include <fcntl.h>
#include <unistd.h>

#include <algorithm>
#include <array>
#include <cerrno>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <limits>
#include <memory>
#include <set>
#include <span>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace ohl::platform::detail {
namespace {

constexpr std::string_view kFinalPrefix = "ohl-tree-";
constexpr std::string_view kStagePrefix = ".ohl-stage-";
constexpr std::string_view kFilesName = "files";
constexpr std::string_view kMarkerName = ".ohl-complete-v1";
constexpr std::string_view kMarkerMagic = "OHLASTG1";
constexpr std::size_t kMaximumStageAttempts = 16;
constexpr std::size_t kMaximumEnumeratedEntries = 100'000;
constexpr std::size_t kMaximumNativeName =
    kFinalPrefix.size() + kMaximumAtomicDirectoryStoreIdentityBytes * 2;

[[nodiscard]] AtomicDirectoryStoreError map_error(
    const int error) noexcept {
  switch (error) {
    case 0:
      return AtomicDirectoryStoreError::none;
    case ENOMEM:
    case EMFILE:
    case ENFILE:
    case ENOSPC:
    case EDQUOT:
      return AtomicDirectoryStoreError::resource_exhausted;
    case ELOOP:
    case ENOTDIR:
    case EACCES:
    case EPERM:
      return AtomicDirectoryStoreError::unsafe_destination;
    case ENOSYS:
    case EOPNOTSUPP:
      return AtomicDirectoryStoreError::unsupported;
    default:
      return AtomicDirectoryStoreError::io_failure;
  }
}

class FileDescriptor final {
 public:
  FileDescriptor() noexcept = default;
  FileDescriptor(NativeOps& ops, const int descriptor) noexcept
      : ops_(&ops), descriptor_(descriptor) {}
  ~FileDescriptor() {
    if (descriptor_ >= 0) {
      (void)ops_->close_fd(descriptor_);
    }
  }

  FileDescriptor(const FileDescriptor&) = delete;
  FileDescriptor& operator=(const FileDescriptor&) = delete;
  FileDescriptor(FileDescriptor&& other) noexcept
      : ops_(std::exchange(other.ops_, nullptr)),
        descriptor_(std::exchange(other.descriptor_, -1)) {}
  FileDescriptor& operator=(FileDescriptor&& other) noexcept {
    if (this != &other) {
      if (descriptor_ >= 0) {
        (void)ops_->close_fd(descriptor_);
      }
      ops_ = std::exchange(other.ops_, nullptr);
      descriptor_ = std::exchange(other.descriptor_, -1);
    }
    return *this;
  }

  [[nodiscard]] int get() const noexcept { return descriptor_; }
  [[nodiscard]] bool valid() const noexcept { return descriptor_ >= 0; }

  [[nodiscard]] AtomicDirectoryStoreError close() noexcept {
    if (descriptor_ < 0) {
      return AtomicDirectoryStoreError::none;
    }
    const auto descriptor = std::exchange(descriptor_, -1);
    return ops_->close_fd(descriptor) == 0
               ? AtomicDirectoryStoreError::none
               : map_error(errno);
  }

 private:
  NativeOps* ops_{nullptr};
  int descriptor_{-1};
};

struct StoredEntry {
  std::vector<std::string> components;
  std::uint64_t size_bytes{0};
};

struct StoredPlan {
  std::string identity;
  std::string final_name;
  std::vector<StoredEntry> entries;
  std::uint64_t total_bytes{0};
};

[[nodiscard]] bool valid_component(const std::string_view component) noexcept {
  return !component.empty() && component != "." && component != ".." &&
         component.find('/') == std::string_view::npos &&
         component.find('\0') == std::string_view::npos;
}

[[nodiscard]] bool owned_private_directory(
    const struct stat& status) noexcept {
  return S_ISDIR(status.st_mode) && status.st_uid == geteuid() &&
         (status.st_mode & (S_IWGRP | S_IWOTH)) == 0;
}

[[nodiscard]] bool same_inode(const struct stat& first,
                              const struct stat& second) noexcept {
  return first.st_dev == second.st_dev && first.st_ino == second.st_ino;
}

[[nodiscard]] std::string hex_encode(const std::span<const std::byte> bytes) {
  constexpr char kHex[] = "0123456789abcdef";
  std::string result(bytes.size() * 2, '0');
  for (std::size_t index = 0; index < bytes.size(); ++index) {
    const auto value = std::to_integer<unsigned int>(bytes[index]);
    result[index * 2] = kHex[value >> 4U];
    result[index * 2 + 1] = kHex[value & 0x0fU];
  }
  return result;
}

[[nodiscard]] std::string identity_name(const std::string_view identity) {
  const auto bytes = std::as_bytes(std::span{identity.data(), identity.size()});
  return std::string{kFinalPrefix} + hex_encode(bytes);
}

[[nodiscard]] std::string path_key(
    const std::span<const std::string> components) {
  std::string result;
  for (std::size_t index = 0; index < components.size(); ++index) {
    if (index != 0) {
      result.push_back('/');
    }
    result += components[index];
  }
  return result;
}

[[nodiscard]] bool copy_plan(const AtomicDirectoryPlan& source,
                             StoredPlan& destination) {
  if (source.identity.empty() ||
      source.identity.size() > kMaximumAtomicDirectoryStoreIdentityBytes) {
    return false;
  }
  destination.identity = std::string{source.identity};
  destination.final_name = identity_name(source.identity);
  destination.entries.clear();
  destination.entries.reserve(source.entries.size());
  destination.total_bytes = 0;
  std::set<std::string> files;
  std::set<std::string> directories;
  for (const auto& source_entry : source.entries) {
    if (source_entry.components.empty() ||
        source_entry.size_bytes >
            std::numeric_limits<std::uint64_t>::max() -
                destination.total_bytes) {
      return false;
    }
    StoredEntry entry;
    entry.components.reserve(source_entry.components.size());
    for (const auto component : source_entry.components) {
      if (!valid_component(component)) {
        return false;
      }
      entry.components.emplace_back(component);
    }
    entry.size_bytes = source_entry.size_bytes;
    const auto key = path_key(entry.components);
    if (files.contains(key) || directories.contains(key)) {
      return false;
    }
    std::string parent;
    for (std::size_t index = 0; index + 1 < entry.components.size(); ++index) {
      if (!parent.empty()) {
        parent.push_back('/');
      }
      parent += entry.components[index];
      if (files.contains(parent)) {
        return false;
      }
      directories.insert(parent);
    }
    files.insert(key);
    destination.total_bytes += source_entry.size_bytes;
    destination.entries.push_back(std::move(entry));
  }
  return true;
}

void append_big_u64(std::vector<std::byte>& bytes,
                    const std::uint64_t value) {
  for (std::size_t index = 0; index < 8; ++index) {
    bytes.push_back(static_cast<std::byte>(
        (value >> ((7U - index) * 8U)) & 0xffU));
  }
}

[[nodiscard]] std::vector<std::byte> marker_bytes(
    const StoredPlan& plan) {
  std::vector<std::byte> result;
  result.reserve(32 + plan.identity.size());
  result.insert(result.end(),
                reinterpret_cast<const std::byte*>(kMarkerMagic.data()),
                reinterpret_cast<const std::byte*>(kMarkerMagic.data() +
                                                   kMarkerMagic.size()));
  append_big_u64(result, static_cast<std::uint64_t>(plan.identity.size()));
  append_big_u64(result, static_cast<std::uint64_t>(plan.entries.size()));
  append_big_u64(result, plan.total_bytes);
  result.insert(result.end(),
                reinterpret_cast<const std::byte*>(plan.identity.data()),
                reinterpret_cast<const std::byte*>(plan.identity.data() +
                                                   plan.identity.size()));
  return result;
}

[[nodiscard]] AtomicDirectoryStoreError write_all(
    NativeOps& ops, const int descriptor,
    const std::span<const std::byte> bytes) noexcept {
  std::size_t offset = 0;
  while (offset < bytes.size()) {
    const auto written =
        ops.write_fd(descriptor, bytes.data() + offset, bytes.size() - offset);
    if (written < 0) {
      if (errno == EINTR) {
        continue;
      }
      return map_error(errno);
    }
    if (written == 0 || std::cmp_greater(written, bytes.size() - offset)) {
      return AtomicDirectoryStoreError::io_failure;
    }
    offset += static_cast<std::size_t>(written);
  }
  return AtomicDirectoryStoreError::none;
}

[[nodiscard]] AtomicDirectoryStoreError read_exact(
    NativeOps& ops, const int descriptor,
    const std::span<std::byte> bytes) noexcept {
  std::size_t offset = 0;
  while (offset < bytes.size()) {
    const auto count =
        ops.read_fd(descriptor, bytes.data() + offset, bytes.size() - offset);
    if (count < 0) {
      if (errno == EINTR) {
        continue;
      }
      return map_error(errno);
    }
    if (count == 0 || std::cmp_greater(count, bytes.size() - offset)) {
      return AtomicDirectoryStoreError::io_failure;
    }
    offset += static_cast<std::size_t>(count);
  }
  std::byte extra{};
  while (true) {
    const auto count = ops.read_fd(descriptor, &extra, 1);
    if (count < 0 && errno == EINTR) {
      continue;
    }
    if (count < 0) {
      return map_error(errno);
    }
    return count == 0 ? AtomicDirectoryStoreError::none
                      : AtomicDirectoryStoreError::io_failure;
  }
}

[[nodiscard]] FileDescriptor open_directory(NativeOps& ops,
                                            const int parent,
                                            const char* name) noexcept {
  return {ops, ops.open_at(parent, name,
                           O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC,
                           0)};
}

struct ExactCheck {
  bool matches{false};
  AtomicDirectoryStoreError error{AtomicDirectoryStoreError::none};
};

[[nodiscard]] ExactCheck exact_marker(NativeOps& ops, const int root,
                                      const StoredPlan& plan) {
  struct stat status {};
  const std::string marker{kMarkerName};
  if (ops.stat_at(root, marker.c_str(), &status, AT_SYMLINK_NOFOLLOW) != 0) {
    return errno == ENOENT ? ExactCheck{} : ExactCheck{false, map_error(errno)};
  }
  if (!S_ISREG(status.st_mode) || status.st_uid != geteuid() ||
      (status.st_mode & (S_IWGRP | S_IWOTH)) != 0 || status.st_nlink != 1) {
    return {};
  }
  const auto expected = marker_bytes(plan);
  if (!std::cmp_equal(status.st_size, expected.size())) {
    return {};
  }
  FileDescriptor descriptor{
      ops, ops.open_at(root, marker.c_str(),
                       O_RDONLY | O_NOFOLLOW | O_CLOEXEC, 0)};
  if (!descriptor.valid()) {
    return {false, map_error(errno)};
  }
  struct stat opened_status {};
  if (ops.stat_fd(descriptor.get(), &opened_status) != 0) {
    return {false, map_error(errno)};
  }
  if (!same_inode(status, opened_status) || !S_ISREG(opened_status.st_mode) ||
      opened_status.st_uid != geteuid() ||
      (opened_status.st_mode & (S_IWGRP | S_IWOTH)) != 0 ||
      opened_status.st_nlink != 1 ||
      !std::cmp_equal(opened_status.st_size, expected.size())) {
    return {};
  }
  std::vector<std::byte> actual(expected.size());
  const auto read_error = read_exact(ops, descriptor.get(), actual);
  return read_error == AtomicDirectoryStoreError::none
             ? ExactCheck{actual == expected, AtomicDirectoryStoreError::none}
             : ExactCheck{false, read_error};
}

struct ActualTree {
  std::set<std::string> directories;
  std::vector<std::pair<std::string, std::uint64_t>> files;
  std::size_t enumerated{0};
};

[[nodiscard]] ExactCheck enumerate_tree(NativeOps& ops, const int directory,
                                        const std::string& prefix,
                                        const dev_t expected_device,
                                        ActualTree& tree) {
  const auto duplicate = ops.duplicate(directory);
  if (duplicate < 0) {
    return {false, map_error(errno)};
  }
  DIR* stream = ops.open_directory_stream(duplicate);
  if (stream == nullptr) {
    (void)ops.close_fd(duplicate);
    return {false, map_error(errno)};
  }
  auto result = ExactCheck{true, AtomicDirectoryStoreError::none};
  while (result.matches && result.error == AtomicDirectoryStoreError::none) {
    errno = 0;
    auto* item = ops.read_directory(stream);
    if (item == nullptr) {
      if (errno != 0) {
        result = {false, map_error(errno)};
      }
      break;
    }
    const std::string_view name{item->d_name};
    if (name == "." || name == "..") {
      continue;
    }
    if (++tree.enumerated > kMaximumEnumeratedEntries ||
        !valid_component(name)) {
      result.matches = false;
      break;
    }
    struct stat status {};
    if (ops.stat_at(directory, item->d_name, &status,
                    AT_SYMLINK_NOFOLLOW) != 0) {
      result = {false, map_error(errno)};
      break;
    }
    const auto path = prefix.empty() ? std::string{name}
                                     : prefix + "/" + std::string{name};
    if (owned_private_directory(status) && status.st_dev == expected_device) {
      auto child = open_directory(ops, directory, item->d_name);
      if (!child.valid()) {
        result = {false, map_error(errno)};
        break;
      }
      struct stat opened_status {};
      if (ops.stat_fd(child.get(), &opened_status) != 0) {
        result = {false, map_error(errno)};
        break;
      }
      if (!owned_private_directory(opened_status) ||
          opened_status.st_dev != expected_device ||
          !same_inode(status, opened_status)) {
        result.matches = false;
        break;
      }
      tree.directories.insert(path);
      result = enumerate_tree(ops, child.get(), path, expected_device, tree);
    } else if (S_ISREG(status.st_mode) && status.st_uid == geteuid() &&
               (status.st_mode & (S_IWGRP | S_IWOTH)) == 0 &&
               status.st_nlink == 1 && status.st_size >= 0) {
      tree.files.emplace_back(path, static_cast<std::uint64_t>(status.st_size));
    } else {
      result.matches = false;
    }
  }
  if (ops.close_directory(stream) != 0) {
    result = {false, map_error(errno)};
  }
  return result;
}

[[nodiscard]] ExactCheck exact_tree(NativeOps& ops, const int final_directory,
                                    const StoredPlan& plan) {
  struct stat final_status {};
  if (ops.stat_fd(final_directory, &final_status) != 0) {
    return {false, map_error(errno)};
  }
  if (!owned_private_directory(final_status)) {
    return {};
  }
  std::set<std::string> root_names;
  {
    const auto duplicate = ops.duplicate(final_directory);
    if (duplicate < 0) {
      return {false, map_error(errno)};
    }
    DIR* stream = ops.open_directory_stream(duplicate);
    if (stream == nullptr) {
      (void)ops.close_fd(duplicate);
      return {false, map_error(errno)};
    }
    auto listing = ExactCheck{true, AtomicDirectoryStoreError::none};
    std::size_t root_entry_count = 0;
    while (listing.matches && listing.error == AtomicDirectoryStoreError::none) {
      errno = 0;
      auto* item = ops.read_directory(stream);
      if (item == nullptr) {
        if (errno != 0) {
          listing = {false, map_error(errno)};
        }
        break;
      }
      const std::string_view name{item->d_name};
      if (name != "." && name != "..") {
        if (++root_entry_count > 2) {
          listing.matches = false;
          break;
        }
        root_names.emplace(name);
      }
    }
    if (ops.close_directory(stream) != 0) {
      return {false, map_error(errno)};
    }
    if (!listing.matches || listing.error != AtomicDirectoryStoreError::none) {
      return listing;
    }
  }
  if (root_names != std::set<std::string>{std::string{kMarkerName},
                                          std::string{kFilesName}}) {
    return {};
  }
  const auto marker = exact_marker(ops, final_directory, plan);
  if (!marker.matches || marker.error != AtomicDirectoryStoreError::none) {
    return marker;
  }
  auto files_directory = open_directory(ops, final_directory,
                                        kFilesName.data());
  if (!files_directory.valid()) {
    return {false, map_error(errno)};
  }
  struct stat files_status {};
  if (ops.stat_fd(files_directory.get(), &files_status) != 0) {
    return {false, map_error(errno)};
  }
  if (!owned_private_directory(files_status) ||
      files_status.st_dev != final_status.st_dev) {
    return {};
  }
  ActualTree actual;
  const auto enumeration = enumerate_tree(
      ops, files_directory.get(), "", final_status.st_dev, actual);
  if (!enumeration.matches ||
      enumeration.error != AtomicDirectoryStoreError::none) {
    return enumeration;
  }
  std::set<std::string> expected_directories;
  std::vector<std::pair<std::string, std::uint64_t>> expected_files;
  for (const auto& entry : plan.entries) {
    std::string parent;
    for (std::size_t index = 0; index + 1 < entry.components.size(); ++index) {
      if (!parent.empty()) {
        parent.push_back('/');
      }
      parent += entry.components[index];
      expected_directories.insert(parent);
    }
    expected_files.emplace_back(path_key(entry.components), entry.size_bytes);
  }
  std::ranges::sort(actual.files);
  std::ranges::sort(expected_files);
  return {actual.directories == expected_directories &&
              actual.files == expected_files,
          AtomicDirectoryStoreError::none};
}

[[nodiscard]] AtomicDirectoryProbeResult probe_plan(
    NativeOps& ops, const int root, const StoredPlan& plan) {
  struct stat status {};
  if (ops.stat_at(root, plan.final_name.c_str(), &status,
                  AT_SYMLINK_NOFOLLOW) != 0) {
    if (errno == ENOENT) {
      return {};
    }
    return {.error = map_error(errno)};
  }
  if (!S_ISDIR(status.st_mode)) {
    return {.state = AtomicDirectoryProbeState::conflict};
  }
  auto final_directory = open_directory(ops, root, plan.final_name.c_str());
  if (!final_directory.valid()) {
    return {.error = map_error(errno)};
  }
  const auto exact = exact_tree(ops, final_directory.get(), plan);
  if (exact.error != AtomicDirectoryStoreError::none) {
    return {.error = exact.error};
  }
  return {.state = exact.matches ? AtomicDirectoryProbeState::matching
                                 : AtomicDirectoryProbeState::conflict};
}

class NativeByteSink final : public AtomicDirectoryByteSink {
 public:
  NativeByteSink(NativeOps& ops, FileDescriptor descriptor,
                 const std::uint64_t expected_size,
                 std::shared_ptr<NativeTransactionLifetime> lifetime,
                 const std::uint64_t generation) noexcept
      : ops_(ops),
        descriptor_(std::move(descriptor)),
        expected_size_(expected_size),
        lifetime_(std::move(lifetime)),
        generation_(generation) {}
  ~NativeByteSink() override { discard(); }

  [[nodiscard]] AtomicDirectoryStoreError write(
      const std::span<const std::byte> bytes) noexcept override {
    if (!native_sink_binding_is_current(lifetime_, generation_, lifetime_) ||
        !descriptor_.valid() ||
        error_ != AtomicDirectoryStoreError::none) {
      return error_ == AtomicDirectoryStoreError::none
                 ? AtomicDirectoryStoreError::invalid_state
                 : error_;
    }
    if (std::cmp_greater(bytes.size(), expected_size_ - bytes_written_)) {
      error_ = AtomicDirectoryStoreError::invalid_state;
      return error_;
    }
    error_ = write_all(ops_, descriptor_.get(), bytes);
    if (error_ == AtomicDirectoryStoreError::none) {
      bytes_written_ += static_cast<std::uint64_t>(bytes.size());
    }
    return error_;
  }

  [[nodiscard]] bool belongs_to_current(
      const std::shared_ptr<NativeTransactionLifetime>& lifetime) const
      noexcept {
    return native_sink_binding_is_current(lifetime_, generation_, lifetime);
  }
  [[nodiscard]] std::uint64_t bytes_written() const noexcept {
    return bytes_written_;
  }
  [[nodiscard]] AtomicDirectoryStoreError error() const noexcept {
    return error_;
  }
  [[nodiscard]] int descriptor() const noexcept { return descriptor_.get(); }
  [[nodiscard]] AtomicDirectoryStoreError close() noexcept {
    const auto result = descriptor_.close();
    close_generation();
    return result;
  }

 private:
  void discard() noexcept {
    if (descriptor_.valid()) {
      (void)descriptor_.close();
    }
    close_generation();
  }

  void close_generation() noexcept {
    if (lifetime_->generation == generation_) {
      lifetime_->open = false;
    }
  }

  NativeOps& ops_;
  FileDescriptor descriptor_;
  std::uint64_t expected_size_{0};
  std::uint64_t bytes_written_{0};
  std::shared_ptr<NativeTransactionLifetime> lifetime_;
  std::uint64_t generation_{0};
  AtomicDirectoryStoreError error_{AtomicDirectoryStoreError::none};
};

[[nodiscard]] AtomicDirectoryStoreError remove_tree(NativeOps& ops,
                                                    const int directory,
                                                    std::size_t& enumerated) {
  const auto duplicate = ops.duplicate(directory);
  if (duplicate < 0) {
    return map_error(errno);
  }
  DIR* stream = ops.open_directory_stream(duplicate);
  if (stream == nullptr) {
    (void)ops.close_fd(duplicate);
    return map_error(errno);
  }
  auto first_error = AtomicDirectoryStoreError::none;
  while (true) {
    errno = 0;
    auto* item = ops.read_directory(stream);
    if (item == nullptr) {
      if (errno != 0 && first_error == AtomicDirectoryStoreError::none) {
        first_error = map_error(errno);
      }
      break;
    }
    const std::string_view name{item->d_name};
    if (name == "." || name == "..") {
      continue;
    }
    if (++enumerated > kMaximumEnumeratedEntries || !valid_component(name)) {
      if (first_error == AtomicDirectoryStoreError::none) {
        first_error = AtomicDirectoryStoreError::unsafe_destination;
      }
      continue;
    }
    struct stat status {};
    if (ops.stat_at(directory, item->d_name, &status,
                    AT_SYMLINK_NOFOLLOW) != 0) {
      if (first_error == AtomicDirectoryStoreError::none) {
        first_error = map_error(errno);
      }
      continue;
    }
    if (S_ISDIR(status.st_mode)) {
      auto child = open_directory(ops, directory, item->d_name);
      if (!child.valid()) {
        if (first_error == AtomicDirectoryStoreError::none) {
          first_error = map_error(errno);
        }
        continue;
      }
      struct stat opened_status {};
      if (ops.stat_fd(child.get(), &opened_status) != 0) {
        if (first_error == AtomicDirectoryStoreError::none) {
          first_error = map_error(errno);
        }
        continue;
      }
      if (!owned_private_directory(opened_status) ||
          !same_inode(status, opened_status)) {
        if (first_error == AtomicDirectoryStoreError::none) {
          first_error = AtomicDirectoryStoreError::unsafe_destination;
        }
        continue;
      }
      const auto child_error = remove_tree(ops, child.get(), enumerated);
      if (first_error == AtomicDirectoryStoreError::none) {
        first_error = child_error;
      }
      const auto child_close_error = child.close();
      if (first_error == AtomicDirectoryStoreError::none) {
        first_error = child_close_error;
      }
      struct stat before_remove {};
      if (ops.stat_at(directory, item->d_name, &before_remove,
                      AT_SYMLINK_NOFOLLOW) != 0) {
        if (first_error == AtomicDirectoryStoreError::none) {
          first_error = map_error(errno);
        }
      } else if (!same_inode(status, before_remove) ||
                 !owned_private_directory(before_remove)) {
        if (first_error == AtomicDirectoryStoreError::none) {
          first_error = AtomicDirectoryStoreError::unsafe_destination;
        }
      } else if (first_error == AtomicDirectoryStoreError::none &&
                 ops.unlink_at(directory, item->d_name, AT_REMOVEDIR) != 0) {
        first_error = map_error(errno);
      }
    } else {
      struct stat before_remove {};
      if (ops.stat_at(directory, item->d_name, &before_remove,
                      AT_SYMLINK_NOFOLLOW) != 0) {
        if (first_error == AtomicDirectoryStoreError::none) {
          first_error = map_error(errno);
        }
      } else if (!same_inode(status, before_remove)) {
        if (first_error == AtomicDirectoryStoreError::none) {
          first_error = AtomicDirectoryStoreError::unsafe_destination;
        }
      } else if (first_error == AtomicDirectoryStoreError::none &&
                 ops.unlink_at(directory, item->d_name, 0) != 0) {
        first_error = map_error(errno);
      }
    }
  }
  if (ops.close_directory(stream) != 0 &&
      first_error == AtomicDirectoryStoreError::none) {
    first_error = map_error(errno);
  }
  return first_error;
}

class NativeTransaction final : public AtomicDirectoryTransaction {
 public:
  NativeTransaction(NativeOps& ops, FileDescriptor root,
                    std::shared_ptr<NativeTransactionLifetime> lifetime)
      noexcept
      : ops_(ops), root_(std::move(root)), lifetime_(std::move(lifetime)) {}
  ~NativeTransaction() override { lifetime_->owner_alive = false; }

  [[nodiscard]] AtomicDirectoryStoreError begin(
      const AtomicDirectoryPlan& plan) noexcept override {
    if (state_ != State::created) {
      return AtomicDirectoryStoreError::invalid_state;
    }
    state_ = State::begin_attempted;
    try {
      if (!copy_plan(plan, plan_)) {
        return AtomicDirectoryStoreError::invalid_state;
      }
    } catch (const std::bad_alloc&) {
      return AtomicDirectoryStoreError::resource_exhausted;
    } catch (...) {
      return AtomicDirectoryStoreError::io_failure;
    }
    std::array<std::byte, 16> random{};
    for (std::size_t attempt = 0; attempt < kMaximumStageAttempts; ++attempt) {
      std::size_t received = 0;
      while (received < random.size()) {
        const auto count = ops_.random_bytes(random.data() + received,
                                             random.size() - received);
        if (count < 0) {
          if (errno == EINTR) {
            continue;
          }
          return map_error(errno);
        }
        if (count == 0 ||
            std::cmp_greater(count, random.size() - received)) {
          return AtomicDirectoryStoreError::io_failure;
        }
        received += static_cast<std::size_t>(count);
      }
      try {
        stage_name_ = std::string{kStagePrefix} + hex_encode(random);
      } catch (const std::bad_alloc&) {
        return AtomicDirectoryStoreError::resource_exhausted;
      }
      if (ops_.make_directory_at(root_.get(), stage_name_.c_str(), 0700) == 0) {
        owns_stage_ = true;
        break;
      }
      if (errno != EEXIST) {
        return map_error(errno);
      }
    }
    if (!owns_stage_) {
      return AtomicDirectoryStoreError::resource_exhausted;
    }
    if (ops_.stat_at(root_.get(), stage_name_.c_str(), &stage_identity_,
                     AT_SYMLINK_NOFOLLOW) != 0) {
      return map_error(errno);
    }
    if (!owned_private_directory(stage_identity_)) {
      return AtomicDirectoryStoreError::unsafe_destination;
    }
    stage_ = open_directory(ops_, root_.get(), stage_name_.c_str());
    if (!stage_.valid()) {
      return map_error(errno);
    }
    struct stat opened_stage {};
    if (ops_.stat_fd(stage_.get(), &opened_stage) != 0) {
      return map_error(errno);
    }
    if (!owned_private_directory(opened_stage) ||
        !same_inode(opened_stage, stage_identity_)) {
      return AtomicDirectoryStoreError::unsafe_destination;
    }
    if (ops_.make_directory_at(stage_.get(), kFilesName.data(), 0700) != 0) {
      return map_error(errno);
    }
    files_ = open_directory(ops_, stage_.get(), kFilesName.data());
    if (!files_.valid()) {
      return map_error(errno);
    }
    struct stat files_status {};
    if (ops_.stat_fd(files_.get(), &files_status) != 0) {
      return map_error(errno);
    }
    if (!owned_private_directory(files_status)) {
      return AtomicDirectoryStoreError::unsafe_destination;
    }
    state_ = State::staging;
    return AtomicDirectoryStoreError::none;
  }

  [[nodiscard]] AtomicDirectoryOpenResult open_file(
      const std::span<const std::string_view> components,
      const std::uint64_t expected_size) noexcept override {
    if (state_ != State::staging || lifetime_->open ||
        next_entry_ >= plan_.entries.size()) {
      return {.sink = nullptr,
              .error = AtomicDirectoryStoreError::invalid_state};
    }
    const auto& expected = plan_.entries[next_entry_];
    if (components.size() != expected.components.size() ||
        expected_size != expected.size_bytes ||
        !std::ranges::equal(components, expected.components)) {
      return {.sink = nullptr,
              .error = AtomicDirectoryStoreError::invalid_state};
    }
    FileDescriptor parent{
        ops_, ops_.duplicate(files_.get())};
    if (!parent.valid()) {
      return {.sink = nullptr, .error = map_error(errno)};
    }
    for (std::size_t index = 0; index + 1 < expected.components.size();
         ++index) {
      const auto& component = expected.components[index];
      bool created = false;
      if (ops_.make_directory_at(parent.get(), component.c_str(), 0700) == 0) {
        created = true;
      } else if (errno != EEXIST) {
        return {.sink = nullptr, .error = map_error(errno)};
      }
      auto child = open_directory(ops_, parent.get(), component.c_str());
      if (!child.valid()) {
        return {.sink = nullptr, .error = map_error(errno)};
      }
      struct stat child_status {};
      if (ops_.stat_fd(child.get(), &child_status) != 0) {
        return {.sink = nullptr, .error = map_error(errno)};
      }
      if (!owned_private_directory(child_status)) {
        return {.sink = nullptr,
                .error = AtomicDirectoryStoreError::unsafe_destination};
      }
      if (created) {
        const auto duplicate = ops_.duplicate(child.get());
        if (duplicate < 0) {
          return {.sink = nullptr, .error = map_error(errno)};
        }
        try {
          directories_to_sync_.emplace_back(ops_, duplicate);
        } catch (const std::bad_alloc&) {
          (void)ops_.close_fd(duplicate);
          return {.sink = nullptr,
                  .error = AtomicDirectoryStoreError::resource_exhausted};
        }
      }
      parent = std::move(child);
    }
    const auto& leaf = expected.components.back();
    FileDescriptor file{
        ops_, ops_.open_at(parent.get(), leaf.c_str(),
                           O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC,
                           0600)};
    if (!file.valid()) {
      return {.sink = nullptr, .error = map_error(errno)};
    }
    if (lifetime_->generation == std::numeric_limits<std::uint64_t>::max()) {
      return {.sink = nullptr,
              .error = AtomicDirectoryStoreError::resource_exhausted};
    }
    ++lifetime_->generation;
    lifetime_->open = true;
    try {
      return {.sink = std::make_unique<NativeByteSink>(
                  ops_, std::move(file), expected_size, lifetime_,
                  lifetime_->generation)};
    } catch (const std::bad_alloc&) {
      lifetime_->open = false;
      return {.sink = nullptr,
              .error = AtomicDirectoryStoreError::resource_exhausted};
    }
  }

  [[nodiscard]] AtomicDirectoryStoreError seal_file(
      std::unique_ptr<AtomicDirectoryByteSink> sink) noexcept override {
    auto* native = dynamic_cast<NativeByteSink*>(sink.get());
    if (state_ != State::staging || !lifetime_->open || native == nullptr ||
        !native->belongs_to_current(lifetime_) ||
        native->error() != AtomicDirectoryStoreError::none ||
        native->bytes_written() != plan_.entries[next_entry_].size_bytes) {
      return AtomicDirectoryStoreError::invalid_state;
    }
    struct stat status {};
    if (ops_.stat_fd(native->descriptor(), &status) != 0) {
      return map_error(errno);
    }
    if (!S_ISREG(status.st_mode) || status.st_nlink != 1 || status.st_size < 0 ||
        static_cast<std::uint64_t>(status.st_size) != native->bytes_written()) {
      return AtomicDirectoryStoreError::unsafe_destination;
    }
    if (ops_.sync_fd(native->descriptor()) != 0) {
      return map_error(errno);
    }
    const auto close_error = native->close();
    sink.reset();
    if (close_error != AtomicDirectoryStoreError::none) {
      return close_error;
    }
    ++next_entry_;
    return AtomicDirectoryStoreError::none;
  }

  [[nodiscard]] AtomicDirectoryStoreError seal_completion(
      const AtomicDirectoryCompletion& completion) noexcept override {
    if (state_ != State::staging || lifetime_->open ||
        next_entry_ != plan_.entries.size() ||
        completion.identity != plan_.identity ||
        !std::cmp_equal(completion.entry_count, plan_.entries.size()) ||
        completion.total_bytes != plan_.total_bytes) {
      return AtomicDirectoryStoreError::invalid_state;
    }
    std::vector<std::byte> marker;
    try {
      marker = marker_bytes(plan_);
    } catch (const std::bad_alloc&) {
      return AtomicDirectoryStoreError::resource_exhausted;
    }
    FileDescriptor marker_file{
        ops_, ops_.open_at(stage_.get(), kMarkerName.data(),
                           O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC,
                           0600)};
    if (!marker_file.valid()) {
      return map_error(errno);
    }
    auto error = write_all(ops_, marker_file.get(), marker);
    if (error == AtomicDirectoryStoreError::none &&
        ops_.sync_fd(marker_file.get()) != 0) {
      error = map_error(errno);
    }
    const auto close_error = marker_file.close();
    if (error == AtomicDirectoryStoreError::none) {
      error = close_error;
    }
    if (error != AtomicDirectoryStoreError::none) {
      return error;
    }
    for (auto iterator = directories_to_sync_.rbegin();
         iterator != directories_to_sync_.rend(); ++iterator) {
      if (ops_.sync_fd(iterator->get()) != 0) {
        return map_error(errno);
      }
    }
    for (auto iterator = directories_to_sync_.rbegin();
         iterator != directories_to_sync_.rend(); ++iterator) {
      const auto close_error = iterator->close();
      if (close_error != AtomicDirectoryStoreError::none) {
        return close_error;
      }
    }
    directories_to_sync_.clear();
    if (ops_.sync_fd(files_.get()) != 0) {
      return map_error(errno);
    }
    const auto files_close_error = files_.close();
    if (files_close_error != AtomicDirectoryStoreError::none) {
      return files_close_error;
    }
    if (ops_.sync_fd(stage_.get()) != 0) {
      return map_error(errno);
    }
    state_ = State::ready;
    return AtomicDirectoryStoreError::none;
  }

  [[nodiscard]] AtomicDirectoryPublishResult publish_no_replace()
      noexcept override {
    if (state_ != State::ready || !owns_stage_) {
      return {.error = AtomicDirectoryStoreError::invalid_state};
    }
    if (ops_.rename_no_replace(root_.get(), stage_name_.c_str(), root_.get(),
                               plan_.final_name.c_str()) == 0) {
      state_ = State::published;
      owns_stage_ = false;
      return {};
    }
    const auto rename_error = errno;
    if (rename_error == EEXIST) {
      return {.state = AtomicDirectoryPublishState::destination_exists};
    }
    if (rename_error == ENOSYS || rename_error == EINVAL ||
        rename_error == EOPNOTSUPP) {
      return {.error = AtomicDirectoryStoreError::unsupported};
    }

    struct stat stage_status {};
    struct stat final_status {};
    const auto stage_result = ops_.stat_at(root_.get(), stage_name_.c_str(),
                                           &stage_status,
                                           AT_SYMLINK_NOFOLLOW);
    const auto stage_error = stage_result == 0 ? 0 : errno;
    const auto final_result = ops_.stat_at(root_.get(), plan_.final_name.c_str(),
                                           &final_status,
                                           AT_SYMLINK_NOFOLLOW);
    const auto final_error = final_result == 0 ? 0 : errno;
    const bool stage_exists = stage_result == 0;
    const bool final_exists = final_result == 0;
    if (stage_exists && final_exists) {
      if (!same_inode(stage_status, stage_identity_)) {
        return {.error = AtomicDirectoryStoreError::unsafe_destination};
      }
      return {.state = AtomicDirectoryPublishState::destination_exists};
    }
    if (stage_exists && same_inode(stage_status, stage_identity_) &&
        !final_exists && final_error == ENOENT) {
      return {.error = map_error(rename_error)};
    }
    if (!stage_exists && stage_error == ENOENT && final_exists) {
      try {
        const auto probe = probe_plan(ops_, root_.get(), plan_);
        if (probe.error == AtomicDirectoryStoreError::none &&
            probe.state == AtomicDirectoryProbeState::matching) {
          state_ = State::published;
          owns_stage_ = false;
          return {};
        }
      } catch (const std::bad_alloc&) {
        return {.error = AtomicDirectoryStoreError::resource_exhausted};
      } catch (...) {
        return {.error = AtomicDirectoryStoreError::io_failure};
      }
    }
    return {.error = AtomicDirectoryStoreError::io_failure};
  }

  [[nodiscard]] AtomicDirectoryStoreError sync_published_parent()
      noexcept override {
    if (state_ != State::published) {
      return AtomicDirectoryStoreError::invalid_state;
    }
    return ops_.sync_fd(root_.get()) == 0
               ? AtomicDirectoryStoreError::none
               : map_error(errno);
  }

  [[nodiscard]] AtomicDirectoryStoreError abort() noexcept override {
    if (state_ == State::published) {
      return AtomicDirectoryStoreError::invalid_state;
    }
    if (state_ == State::aborted) {
      return abort_error_;
    }
    if (lifetime_->open) {
      return AtomicDirectoryStoreError::invalid_state;
    }
    if (!owns_stage_) {
      state_ = State::aborted;
      return AtomicDirectoryStoreError::none;
    }
    struct stat current {};
    if (ops_.stat_at(root_.get(), stage_name_.c_str(), &current,
                     AT_SYMLINK_NOFOLLOW) != 0 ||
        !S_ISDIR(current.st_mode) || !same_inode(current, stage_identity_)) {
      abort_error_ = AtomicDirectoryStoreError::unsafe_destination;
      state_ = State::aborted;
      return abort_error_;
    }
    std::size_t enumerated = 0;
    abort_error_ = stage_.valid()
                       ? remove_tree(ops_, stage_.get(), enumerated)
                       : AtomicDirectoryStoreError::none;
    const auto files_close_error = files_.close();
    if (abort_error_ == AtomicDirectoryStoreError::none) {
      abort_error_ = files_close_error;
    }
    const auto stage_close_error = stage_.close();
    if (abort_error_ == AtomicDirectoryStoreError::none) {
      abort_error_ = stage_close_error;
    }
    if (ops_.stat_at(root_.get(), stage_name_.c_str(), &current,
                     AT_SYMLINK_NOFOLLOW) != 0 ||
        !S_ISDIR(current.st_mode) || !same_inode(current, stage_identity_)) {
      if (abort_error_ == AtomicDirectoryStoreError::none) {
        abort_error_ = AtomicDirectoryStoreError::unsafe_destination;
      }
    } else if (abort_error_ == AtomicDirectoryStoreError::none &&
               ops_.unlink_at(root_.get(), stage_name_.c_str(),
                              AT_REMOVEDIR) != 0) {
      abort_error_ = map_error(errno);
    }
    if (abort_error_ == AtomicDirectoryStoreError::none) {
      owns_stage_ = false;
    }
    state_ = State::aborted;
    return abort_error_;
  }

 private:
  enum class State { created, begin_attempted, staging, ready, published, aborted };

  NativeOps& ops_;
  FileDescriptor root_;
  std::shared_ptr<NativeTransactionLifetime> lifetime_;
  FileDescriptor stage_;
  FileDescriptor files_;
  std::vector<FileDescriptor> directories_to_sync_;
  StoredPlan plan_;
  std::string stage_name_;
  struct stat stage_identity_ {};
  std::size_t next_entry_{0};
  bool owns_stage_{false};
  State state_{State::created};
  AtomicDirectoryStoreError abort_error_{AtomicDirectoryStoreError::none};
};

class NativeStore final : public AtomicDirectoryStore {
 public:
  NativeStore(NativeOps& ops, FileDescriptor root) noexcept
      : ops_(ops), root_(std::move(root)) {}

  [[nodiscard]] AtomicDirectoryProbeResult probe(
      const AtomicDirectoryPlan& plan) noexcept override {
    try {
      StoredPlan copy;
      if (!copy_plan(plan, copy)) {
        return {.error = AtomicDirectoryStoreError::invalid_state};
      }
      return probe_plan(ops_, root_.get(), copy);
    } catch (const std::bad_alloc&) {
      return {.error = AtomicDirectoryStoreError::resource_exhausted};
    } catch (...) {
      return {.error = AtomicDirectoryStoreError::io_failure};
    }
  }

  [[nodiscard]] AtomicDirectoryTransactionResult create_transaction()
      noexcept override {
    const auto duplicate = ops_.duplicate(root_.get());
    if (duplicate < 0) {
      return {.transaction = nullptr, .error = map_error(errno)};
    }
    FileDescriptor root_copy{ops_, duplicate};
    try {
      auto lifetime = std::make_shared<NativeTransactionLifetime>();
      return {.transaction = std::make_unique<NativeTransaction>(
                  ops_, std::move(root_copy), std::move(lifetime))};
    } catch (const std::bad_alloc&) {
      return {.transaction = nullptr,
              .error = AtomicDirectoryStoreError::resource_exhausted};
    }
  }

 private:
  NativeOps& ops_;
  FileDescriptor root_;
};

}  // namespace

AtomicDirectoryStoreOpenResult open_atomic_directory_store_with_ops(
    const std::filesystem::path& root, NativeOps& ops) noexcept {
  try {
    if (root.empty() || !root.is_absolute() || root != root.lexically_normal()) {
      return {.store = nullptr,
              .error = AtomicDirectoryStoreError::unsafe_destination};
    }
    const auto native = root.native();
    if (native.find('\0') != std::string::npos) {
      return {.store = nullptr,
              .error = AtomicDirectoryStoreError::unsafe_destination};
    }
    FileDescriptor current{ops, ops.open_root()};
    if (!current.valid()) {
      return {.store = nullptr, .error = map_error(errno)};
    }
    for (const auto& part : root.relative_path()) {
      const auto component = part.native();
      if (!valid_component(component)) {
        return {.store = nullptr,
                .error = AtomicDirectoryStoreError::unsafe_destination};
      }
      auto next = open_directory(ops, current.get(), component.c_str());
      if (!next.valid()) {
        return {.store = nullptr, .error = map_error(errno)};
      }
      current = std::move(next);
    }
    struct stat status {};
    if (ops.stat_fd(current.get(), &status) != 0) {
      return {.store = nullptr, .error = map_error(errno)};
    }
    if (!S_ISDIR(status.st_mode) || status.st_uid != geteuid() ||
        (status.st_mode & (S_IWGRP | S_IWOTH)) != 0) {
      return {.store = nullptr,
              .error = AtomicDirectoryStoreError::unsafe_destination};
    }
    struct statfs filesystem_status {};
    if (ops.stat_filesystem(current.get(), &filesystem_status) != 0) {
      return {.store = nullptr, .error = map_error(errno)};
    }
    if (!ops.supported_filesystem(filesystem_status)) {
      return {.store = nullptr,
              .error = AtomicDirectoryStoreError::unsupported};
    }
    errno = 0;
    const auto maximum_name = ops.name_max(current.get());
    if (maximum_name < 0) {
      return {.store = nullptr,
              .error = errno == 0 ? AtomicDirectoryStoreError::unsupported
                                  : map_error(errno)};
    }
    if (maximum_name < static_cast<long>(kMaximumNativeName)) {
      return {.store = nullptr,
              .error = AtomicDirectoryStoreError::unsupported};
    }
    try {
      return {.store =
                  std::make_unique<NativeStore>(ops, std::move(current))};
    } catch (const std::bad_alloc&) {
      return {.store = nullptr,
              .error = AtomicDirectoryStoreError::resource_exhausted};
    }
  } catch (const std::bad_alloc&) {
    return {.store = nullptr,
            .error = AtomicDirectoryStoreError::resource_exhausted};
  } catch (...) {
    return {.store = nullptr, .error = AtomicDirectoryStoreError::io_failure};
  }
}

}  // namespace ohl::platform::detail

#endif
