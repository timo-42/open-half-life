#pragma once

#include "ohl/platform/atomic_directory_store.hpp"

#if defined(__linux__)

#include <dirent.h>
#include <sys/stat.h>
#include <sys/statfs.h>
#include <sys/types.h>

#include <cstddef>
#include <cstdint>
#include <filesystem>
#include <memory>

namespace ohl::platform::detail {

struct NativeTransactionLifetime {
  std::uint64_t generation{0};
  bool open{false};
  bool owner_alive{true};
};

[[nodiscard]] inline bool native_sink_binding_is_current(
    const std::shared_ptr<NativeTransactionLifetime>& sink_lifetime,
    const std::uint64_t sink_generation,
    const std::shared_ptr<NativeTransactionLifetime>& transaction_lifetime)
    noexcept {
  return transaction_lifetime != nullptr &&
         sink_lifetime == transaction_lifetime &&
         sink_generation == transaction_lifetime->generation &&
         transaction_lifetime->owner_alive && transaction_lifetime->open;
}

class NativeOps {
 public:
  virtual ~NativeOps() = default;

  virtual int open_root() noexcept = 0;
  virtual int open_at(int directory, const char* name, int flags,
                      mode_t mode) noexcept = 0;
  virtual int make_directory_at(int directory, const char* name,
                                mode_t mode) noexcept = 0;
  virtual int unlink_at(int directory, const char* name, int flags) noexcept = 0;
  virtual int duplicate(int descriptor) noexcept = 0;
  virtual int close_fd(int descriptor) noexcept = 0;
  virtual int stat_fd(int descriptor, struct stat* status) noexcept = 0;
  virtual int stat_at(int directory, const char* name, struct stat* status,
                      int flags) noexcept = 0;
  virtual int stat_filesystem(int descriptor,
                              struct statfs* status) noexcept = 0;
  virtual bool supported_filesystem(const struct statfs& status) noexcept = 0;
  virtual long name_max(int descriptor) noexcept = 0;
  virtual ssize_t read_fd(int descriptor, void* data,
                          std::size_t size) noexcept = 0;
  virtual ssize_t write_fd(int descriptor, const void* data,
                           std::size_t size) noexcept = 0;
  virtual int sync_fd(int descriptor) noexcept = 0;
  virtual ssize_t random_bytes(void* data, std::size_t size) noexcept = 0;
  virtual int rename_no_replace(int old_directory, const char* old_name,
                                int new_directory,
                                const char* new_name) noexcept = 0;
  virtual DIR* open_directory_stream(int descriptor) noexcept = 0;
  virtual struct dirent* read_directory(DIR* directory) noexcept = 0;
  virtual int close_directory(DIR* directory) noexcept = 0;
};

[[nodiscard]] AtomicDirectoryStoreOpenResult
open_atomic_directory_store_with_ops(const std::filesystem::path& root,
                                     NativeOps& ops) noexcept;

[[nodiscard]] NativeOps& linux_native_ops() noexcept;

}  // namespace ohl::platform::detail

#endif
