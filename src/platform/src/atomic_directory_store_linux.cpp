#include "atomic_directory_store_internal.hpp"

#if defined(__linux__)

#include "ohl/platform/atomic_directory_store.hpp"

#include <fcntl.h>
#include <linux/magic.h>
#include <sys/random.h>
#include <sys/syscall.h>
#include <unistd.h>

#include <cerrno>
#include <cstddef>
#include <cstdint>
#include <filesystem>

namespace ohl::platform::detail {
namespace {

// ext2, ext3, and ext4 intentionally share this statfs magic value. A statfs
// probe cannot distinguish the individual ext-family filesystem version.
constexpr long kExtFamilySuperMagic = 0xEF53;

class LinuxNativeOps final : public NativeOps {
 public:
  int open_root() noexcept override {
    return ::open("/", O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
  }

  int open_at(const int directory, const char* name, const int flags,
              const mode_t mode) noexcept override {
    return ::openat(directory, name, flags, mode);
  }

  int make_directory_at(const int directory, const char* name,
                        const mode_t mode) noexcept override {
    return ::mkdirat(directory, name, mode);
  }

  int unlink_at(const int directory, const char* name,
                const int flags) noexcept override {
    return ::unlinkat(directory, name, flags);
  }

  int duplicate(const int descriptor) noexcept override {
    return ::fcntl(descriptor, F_DUPFD_CLOEXEC, 0);
  }

  int close_fd(const int descriptor) noexcept override {
    return ::close(descriptor);
  }

  int stat_fd(const int descriptor, struct stat* status) noexcept override {
    return ::fstat(descriptor, status);
  }

  int stat_at(const int directory, const char* name, struct stat* status,
              const int flags) noexcept override {
    return ::fstatat(directory, name, status, flags);
  }

  int stat_filesystem(const int descriptor,
                      struct statfs* status) noexcept override {
    return ::fstatfs(descriptor, status);
  }

  bool supported_filesystem(const struct statfs& status) noexcept override {
    return status.f_type == kExtFamilySuperMagic ||
           status.f_type == XFS_SUPER_MAGIC ||
           status.f_type == BTRFS_SUPER_MAGIC || status.f_type == TMPFS_MAGIC;
  }

  long name_max(const int descriptor) noexcept override {
    return ::fpathconf(descriptor, _PC_NAME_MAX);
  }

  ssize_t read_fd(const int descriptor, void* data,
                  const std::size_t size) noexcept override {
    return ::read(descriptor, data, size);
  }

  ssize_t write_fd(const int descriptor, const void* data,
                   const std::size_t size) noexcept override {
    return ::write(descriptor, data, size);
  }

  int sync_fd(const int descriptor) noexcept override {
    return ::fsync(descriptor);
  }

  ssize_t random_bytes(void* data, const std::size_t size) noexcept override {
    return ::getrandom(data, size, 0);
  }

  int rename_no_replace(const int old_directory, const char* old_name,
                        const int new_directory,
                        const char* new_name) noexcept override {
#if defined(SYS_renameat2)
    return static_cast<int>(::syscall(SYS_renameat2, old_directory, old_name,
                                      new_directory, new_name,
                                      RENAME_NOREPLACE));
#else
    (void)old_directory;
    (void)old_name;
    (void)new_directory;
    (void)new_name;
    errno = ENOSYS;
    return -1;
#endif
  }

  DIR* open_directory_stream(const int descriptor) noexcept override {
    return ::fdopendir(descriptor);
  }

  struct dirent* read_directory(DIR* directory) noexcept override {
    return ::readdir(directory);
  }

  int close_directory(DIR* directory) noexcept override {
    return ::closedir(directory);
  }
};

}  // namespace

NativeOps& linux_native_ops() noexcept {
  static LinuxNativeOps ops;
  return ops;
}

}  // namespace ohl::platform::detail

namespace ohl::platform {

AtomicDirectoryStoreOpenResult open_atomic_directory_store(
    const std::filesystem::path& root) noexcept {
  return detail::open_atomic_directory_store_with_ops(
      root, detail::linux_native_ops());
}

}  // namespace ohl::platform

#endif
