#include "isolated_worker_internal.hpp"

#include "isolated_worker_windows_internal.hpp"

#ifndef NOMINMAX
#define NOMINMAX
#endif
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>

#include <bcrypt.h>
#include <userenv.h>

#include <algorithm>
#include <array>
#include <atomic>
#include <chrono>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <cwchar>
#include <limits>
#include <memory>
#include <new>
#include <span>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace ohl::platform::detail {
namespace {

using Clock = std::chrono::steady_clock;
using windows::kBootstrapReady;

#ifdef OHL_WINDOWS_ISOLATED_WORKER_TESTING
std::atomic<windows::IsolatedWorkerLaunchStage> g_last_launch_stage{
    windows::IsolatedWorkerLaunchStage::idle};
std::atomic<windows::IsolatedWorkerCreateProcessFailure>
    g_last_create_process_failure{
        windows::IsolatedWorkerCreateProcessFailure::none};
std::atomic<DWORD> g_last_create_process_error{ERROR_SUCCESS};

void record_launch_stage(
    const windows::IsolatedWorkerLaunchStage stage) noexcept {
  g_last_launch_stage.store(stage);
}

[[nodiscard]] windows::IsolatedWorkerCreateProcessFailure
classify_create_process_failure(const DWORD error) noexcept {
  switch (error) {
    case ERROR_INVALID_PARAMETER:
    case ERROR_BAD_LENGTH:
      return windows::IsolatedWorkerCreateProcessFailure::invalid_parameter;
    case ERROR_ACCESS_DENIED:
    case ERROR_PRIVILEGE_NOT_HELD:
      return windows::IsolatedWorkerCreateProcessFailure::
          access_denied_or_job_nesting;
    case ERROR_NOT_SUPPORTED:
    case ERROR_CALL_NOT_IMPLEMENTED:
    case ERROR_PROC_NOT_FOUND:
      return windows::IsolatedWorkerCreateProcessFailure::unsupported_attribute;
    case ERROR_BAD_EXE_FORMAT:
    case ERROR_BAD_FORMAT:
    case ERROR_INVALID_EXE_SIGNATURE:
    case ERROR_EXE_MARKED_INVALID:
    case ERROR_EXE_MACHINE_TYPE_MISMATCH:
      return windows::IsolatedWorkerCreateProcessFailure::bad_image;
    case ERROR_FILE_NOT_FOUND:
    case ERROR_PATH_NOT_FOUND:
    case ERROR_INVALID_NAME:
    case ERROR_FILENAME_EXCED_RANGE:
    case ERROR_DIRECTORY:
      return windows::IsolatedWorkerCreateProcessFailure::file_or_path;
    default:
      return windows::IsolatedWorkerCreateProcessFailure::other;
  }
}
#endif
[[nodiscard]] bool delete_profile_once(const std::wstring& name) noexcept {
  const HRESULT result = DeleteAppContainerProfile(name.c_str());
  return SUCCEEDED(result) ||
         result == HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND) ||
         result == HRESULT_FROM_WIN32(ERROR_PATH_NOT_FOUND);
}

class UniqueHandle final {
 public:
  UniqueHandle() noexcept = default;
  explicit UniqueHandle(const HANDLE handle) noexcept : handle_(handle) {}

  ~UniqueHandle() { reset(); }

  UniqueHandle(const UniqueHandle&) = delete;
  UniqueHandle& operator=(const UniqueHandle&) = delete;

  UniqueHandle(UniqueHandle&& other) noexcept : handle_(other.release()) {}
  UniqueHandle& operator=(UniqueHandle&& other) noexcept {
    if (this != &other) {
      reset(other.release());
    }
    return *this;
  }

  [[nodiscard]] HANDLE get() const noexcept { return handle_; }
  [[nodiscard]] explicit operator bool() const noexcept {
    return handle_ != nullptr && handle_ != INVALID_HANDLE_VALUE;
  }

  [[nodiscard]] HANDLE release() noexcept {
    const HANDLE result = handle_;
    handle_ = nullptr;
    return result;
  }

  void reset(const HANDLE replacement = nullptr) noexcept {
    if (*this) {
      CloseHandle(handle_);
    }
    handle_ = replacement;
  }

 private:
  HANDLE handle_{nullptr};
};

class UniqueSid final {
 public:
  UniqueSid() noexcept = default;
  explicit UniqueSid(PSID sid) noexcept : sid_(sid) {}
  ~UniqueSid() {
    if (sid_ != nullptr) {
      FreeSid(sid_);
    }
  }

  UniqueSid(const UniqueSid&) = delete;
  UniqueSid& operator=(const UniqueSid&) = delete;
  UniqueSid(UniqueSid&& other) noexcept : sid_(other.release()) {}
  UniqueSid& operator=(UniqueSid&& other) noexcept {
    if (this != &other) {
      if (sid_ != nullptr) {
        FreeSid(sid_);
      }
      sid_ = other.release();
    }
    return *this;
  }

  [[nodiscard]] PSID get() const noexcept { return sid_; }
  [[nodiscard]] PSID release() noexcept {
    PSID result = sid_;
    sid_ = nullptr;
    return result;
  }

 private:
  PSID sid_{nullptr};
};

struct ProfileRecord final {
  explicit ProfileRecord(std::wstring profile_name) noexcept
      : name(std::move(profile_name)) {}

  std::wstring name;
  ProfileRecord* next{nullptr};
};

// Failed deletions retain the opaque randomly generated name for a later
// retry. No profile name or native error escapes through the public API.
class PendingProfileRegistry final {
 public:
  ~PendingProfileRegistry() {
    while (head_ != nullptr) {
      ProfileRecord* const current = head_;
      head_ = current->next;
      static_cast<void>(delete_profile_once(current->name));
      static_cast<void>(delete_profile_once(current->name));
      delete current;
    }
  }

  PendingProfileRegistry(const PendingProfileRegistry&) = delete;
  PendingProfileRegistry& operator=(const PendingProfileRegistry&) = delete;

  [[nodiscard]] bool retry_all() noexcept {
    AcquireSRWLockExclusive(&lock_);
    ProfileRecord** link = &head_;
    while (*link != nullptr) {
      ProfileRecord* const current = *link;
      if (delete_profile_once(current->name) ||
          delete_profile_once(current->name)) {
        *link = current->next;
        delete current;
      } else {
        link = &current->next;
      }
    }
    const bool empty = head_ == nullptr;
    ReleaseSRWLockExclusive(&lock_);
    return empty;
  }

  void retain(std::unique_ptr<ProfileRecord> record) noexcept {
    if (record == nullptr) {
      return;
    }
    AcquireSRWLockExclusive(&lock_);
    record->next = head_;
    head_ = record.release();
    ReleaseSRWLockExclusive(&lock_);
  }

  static PendingProfileRegistry& instance() noexcept {
    static PendingProfileRegistry registry;
    return registry;
  }

 private:
  PendingProfileRegistry() noexcept = default;

  SRWLOCK lock_ = SRWLOCK_INIT;
  ProfileRecord* head_{nullptr};
};

class ProfileGuard final {
 public:
  explicit ProfileGuard(std::unique_ptr<ProfileRecord> record) noexcept
      : record_(std::move(record)) {}
  ~ProfileGuard() {
    if (armed_ && record_ != nullptr && !cleanup()) {
      PendingProfileRegistry::instance().retain(std::move(record_));
    }
  }

  ProfileGuard(const ProfileGuard&) = delete;
  ProfileGuard& operator=(const ProfileGuard&) = delete;

  void arm() noexcept { armed_ = true; }

  [[nodiscard]] bool cleanup() noexcept {
    if (!armed_ || record_ == nullptr) {
      return true;
    }
    // Microsoft documents the state as indeterminate after a failed deletion
    // and directs callers to retry the same operation.
    if (!delete_profile_once(record_->name) &&
        !delete_profile_once(record_->name)) {
      return false;
    }
    armed_ = false;
    record_.reset();
    return true;
  }

  [[nodiscard]] std::unique_ptr<ProfileRecord> release() noexcept {
    armed_ = false;
    return std::move(record_);
  }

  [[nodiscard]] bool cleanup_or_defer() noexcept {
    if (cleanup()) {
      return true;
    }
    armed_ = false;
    PendingProfileRegistry::instance().retain(std::move(record_));
    return false;
  }

 private:
  std::unique_ptr<ProfileRecord> record_;
  bool armed_{false};
};

class ProcessCleanupGuard final {
 public:
  ProcessCleanupGuard(const HANDLE process, const HANDLE job) noexcept
      : process_(process), job_(job) {}
  ~ProcessCleanupGuard() {
    if (!active_) {
      return;
    }
    if (job_ != nullptr) {
      static_cast<void>(
          TerminateJobObject(job_, windows::kTerminationExitCode));
    }
    if (process_ != nullptr) {
      // The direct termination is a fallback for the verification-failure case
      // where the process unexpectedly was not assigned to the private job.
      static_cast<void>(
          TerminateProcess(process_, windows::kTerminationExitCode));
      static_cast<void>(WaitForSingleObject(process_, INFINITE));
    }
  }

  ProcessCleanupGuard(const ProcessCleanupGuard&) = delete;
  ProcessCleanupGuard& operator=(const ProcessCleanupGuard&) = delete;

  void dismiss() noexcept { active_ = false; }

 private:
  HANDLE process_{nullptr};
  HANDLE job_{nullptr};
  bool active_{true};
};

class AttributeList final {
 public:
  AttributeList() noexcept = default;
  ~AttributeList() {
    if (list_ != nullptr) {
      DeleteProcThreadAttributeList(list_);
      HeapFree(GetProcessHeap(), 0, storage_);
    }
  }

  AttributeList(const AttributeList&) = delete;
  AttributeList& operator=(const AttributeList&) = delete;

  [[nodiscard]] bool initialize(const DWORD attribute_count) noexcept {
    SIZE_T bytes = 0;
    static_cast<void>(
        InitializeProcThreadAttributeList(nullptr, attribute_count, 0, &bytes));
    if (GetLastError() != ERROR_INSUFFICIENT_BUFFER || bytes == 0) {
      return false;
    }
    storage_ = HeapAlloc(GetProcessHeap(), HEAP_ZERO_MEMORY, bytes);
    if (storage_ == nullptr) {
      return false;
    }
    list_ = static_cast<LPPROC_THREAD_ATTRIBUTE_LIST>(storage_);
    if (!InitializeProcThreadAttributeList(list_, attribute_count, 0, &bytes)) {
      HeapFree(GetProcessHeap(), 0, storage_);
      storage_ = nullptr;
      list_ = nullptr;
      return false;
    }
    return true;
  }

  template <typename T>
  [[nodiscard]] bool set(const DWORD_PTR key, T& value) noexcept {
    return UpdateProcThreadAttribute(list_, 0, key, &value, sizeof(value),
                                     nullptr, nullptr) != FALSE;
  }

  template <typename T, std::size_t Extent>
  [[nodiscard]] bool set_span(const DWORD_PTR key,
                              std::span<T, Extent> values) noexcept {
    return UpdateProcThreadAttribute(list_, 0, key, values.data(),
                                     values.size_bytes(), nullptr,
                                     nullptr) != FALSE;
  }

  [[nodiscard]] LPPROC_THREAD_ATTRIBUTE_LIST get() const noexcept {
    return list_;
  }

 private:
  void* storage_{nullptr};
  LPPROC_THREAD_ATTRIBUTE_LIST list_{nullptr};
};

struct TrustedExecutable final {
  std::wstring path;
  std::wstring directory;
  UniqueHandle host_image;
  UniqueHandle installation_directory;
  UniqueHandle file;
  FILE_ID_INFO host_identity{};
  FILE_ID_INFO identity{};
};

struct PipePair final {
  UniqueHandle server;
  UniqueHandle child;
};

class PendingOverlappedGuard final {
 public:
  PendingOverlappedGuard(const HANDLE handle, OVERLAPPED& operation,
                         const bool pending) noexcept
      : handle_(handle), operation_(&operation), pending_(pending) {}

  ~PendingOverlappedGuard() {
    if (pending_) {
      static_cast<void>(CancelIoEx(handle_, operation_));
      DWORD ignored = 0;
      static_cast<void>(
          GetOverlappedResult(handle_, operation_, &ignored, TRUE));
    }
  }

  PendingOverlappedGuard(const PendingOverlappedGuard&) = delete;
  PendingOverlappedGuard& operator=(const PendingOverlappedGuard&) = delete;

  [[nodiscard]] bool complete() noexcept {
    if (!pending_) {
      return true;
    }
    DWORD ignored = 0;
    const BOOL result =
        GetOverlappedResult(handle_, operation_, &ignored, TRUE);
    // A blocking GetOverlappedResult resolves the operation even when its
    // final status is cancellation or another error.
    pending_ = false;
    return result != FALSE;
  }

 private:
  HANDLE handle_;
  OVERLAPPED* operation_;
  bool pending_;
};

SRWLOCK g_inheritable_handle_window_lock = SRWLOCK_INIT;

// HANDLE_LIST still requires listed handles to carry HANDLE_FLAG_INHERIT.
// Keep that process-global bit set only around CreateProcessW and serialize
// this backend's launch windows so concurrent worker launches cannot leak one
// another's channel handles.
class ScopedInheritableHandleWindow final {
 public:
  explicit ScopedInheritableHandleWindow(UniqueHandle& handle) noexcept
      : handle_(&handle) {
    AcquireSRWLockExclusive(&g_inheritable_handle_window_lock);
    locked_ = true;
    if (!SetHandleInformation(handle_->get(), HANDLE_FLAG_INHERIT,
                              HANDLE_FLAG_INHERIT)) {
      ReleaseSRWLockExclusive(&g_inheritable_handle_window_lock);
      locked_ = false;
    }
  }

  ~ScopedInheritableHandleWindow() {
    if (!locked_) {
      return;
    }
    if (*handle_ &&
        !SetHandleInformation(handle_->get(), HANDLE_FLAG_INHERIT, 0)) {
      // Closing under the lock is the only fail-closed response if the
      // process-global inheritance bit cannot be cleared.
      handle_->reset();
    }
    ReleaseSRWLockExclusive(&g_inheritable_handle_window_lock);
  }

  ScopedInheritableHandleWindow(const ScopedInheritableHandleWindow&) = delete;
  ScopedInheritableHandleWindow& operator=(
      const ScopedInheritableHandleWindow&) = delete;

  [[nodiscard]] bool valid() const noexcept { return locked_; }

  void close_parent_copy() noexcept {
    if (locked_ && *handle_) {
      handle_->reset();
    }
  }

 private:
  UniqueHandle* handle_;
  bool locked_{false};
};

[[nodiscard]] DWORD wait_milliseconds(
    const Clock::time_point deadline) noexcept {
  if (deadline == Clock::time_point::max()) {
    return INFINITE;
  }
  const auto now = Clock::now();
  if (deadline <= now) {
    return 0;
  }
  const auto remaining = deadline - now;
  auto milliseconds =
      std::chrono::duration_cast<std::chrono::milliseconds>(remaining);
  if (milliseconds < remaining) {
    ++milliseconds;
  }
  constexpr auto maximum =
      static_cast<std::int64_t>(std::numeric_limits<DWORD>::max() - 1U);
  return static_cast<DWORD>(
      std::min<std::int64_t>(milliseconds.count(), maximum));
}

[[nodiscard]] bool random_bytes(const std::span<std::byte> destination) noexcept {
  if (destination.size() > std::numeric_limits<ULONG>::max()) {
    return false;
  }
  return BCryptGenRandom(nullptr,
                         reinterpret_cast<PUCHAR>(destination.data()),
                         static_cast<ULONG>(destination.size()),
                         BCRYPT_USE_SYSTEM_PREFERRED_RNG) == 0;
}

[[nodiscard]] std::wstring random_suffix() {
  std::array<std::byte, 16> random{};
  if (!random_bytes(random)) {
    return {};
  }
  constexpr wchar_t hex[] = L"0123456789abcdef";
  std::wstring result;
  result.reserve(random.size() * 2U);
  for (const std::byte value : random) {
    const auto byte = static_cast<unsigned int>(value);
    result.push_back(hex[(byte >> 4U) & 0xFU]);
    result.push_back(hex[byte & 0xFU]);
  }
  return result;
}

[[nodiscard]] bool query_file_identity(const HANDLE file,
                                       FILE_ID_INFO& identity) noexcept {
  return GetFileInformationByHandleEx(file, FileIdInfo, &identity,
                                      sizeof(identity)) != FALSE;
}

[[nodiscard]] bool query_file_standard_information(
    const HANDLE file, FILE_STANDARD_INFO& information) noexcept {
  return GetFileInformationByHandleEx(file, FileStandardInfo, &information,
                                      sizeof(information)) != FALSE;
}

[[nodiscard]] bool final_path_for_handle(const HANDLE file,
                                         std::wstring& path) {
  const DWORD required = GetFinalPathNameByHandleW(
      file, nullptr, 0, FILE_NAME_NORMALIZED | VOLUME_NAME_DOS);
  if (required == 0) {
    return false;
  }
  std::wstring storage(static_cast<std::size_t>(required), L'\0');
  const DWORD length = GetFinalPathNameByHandleW(
      file, storage.data(), required,
      FILE_NAME_NORMALIZED | VOLUME_NAME_DOS);
  if (length == 0 || length >= required) {
    return false;
  }
  storage.resize(length);
  path = std::move(storage);
  return true;
}

[[nodiscard]] bool same_file_identity(const FILE_ID_INFO& left,
                                      const FILE_ID_INFO& right) noexcept {
  return left.VolumeSerialNumber == right.VolumeSerialNumber &&
         std::memcmp(&left.FileId, &right.FileId, sizeof(left.FileId)) == 0;
}

[[nodiscard]] IsolatedWorkerError locate_trusted_executable(
    const IsolatedWorkerService service, TrustedExecutable& result) {
  if (service != IsolatedWorkerService::media_parser) {
    return IsolatedWorkerError::invalid_argument;
  }

  std::wstring module_path(32'768U, L'\0');
  const DWORD length = GetModuleFileNameW(
      nullptr, module_path.data(), static_cast<DWORD>(module_path.size()));
  if (length == 0 ||
      static_cast<std::size_t>(length) >= module_path.size()) {
    return IsolatedWorkerError::service_identity_mismatch;
  }
  module_path.resize(length);
  const std::size_t separator = module_path.find_last_of(L"\\/");
  if (separator == std::wstring::npos) {
    return IsolatedWorkerError::service_identity_mismatch;
  }

  result.directory = module_path.substr(0, separator);
  const std::wstring worker_path =
      result.directory + L'\\' +
      std::wstring{windows::kMediaParserExecutableName};

  result.host_image.reset(CreateFileW(
      module_path.c_str(), FILE_READ_ATTRIBUTES | READ_CONTROL,
      FILE_SHARE_READ, nullptr, OPEN_EXISTING,
      FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT, nullptr));
  result.installation_directory.reset(CreateFileW(
      result.directory.c_str(), FILE_READ_ATTRIBUTES | READ_CONTROL,
      FILE_SHARE_READ, nullptr, OPEN_EXISTING,
      FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT, nullptr));
  FILE_ATTRIBUTE_TAG_INFO host_tag{};
  FILE_ATTRIBUTE_TAG_INFO directory_tag{};
  FILE_STANDARD_INFO host_standard{};
  FILE_STANDARD_INFO directory_standard{};
  if (!result.host_image || !result.installation_directory ||
      !query_file_identity(result.host_image.get(), result.host_identity) ||
      !GetFileInformationByHandleEx(result.host_image.get(),
                                    FileAttributeTagInfo, &host_tag,
                                    sizeof(host_tag)) ||
      !GetFileInformationByHandleEx(result.installation_directory.get(),
                                    FileAttributeTagInfo, &directory_tag,
                                    sizeof(directory_tag)) ||
      !query_file_standard_information(result.host_image.get(),
                                       host_standard) ||
      !query_file_standard_information(result.installation_directory.get(),
                                       directory_standard) ||
      (host_tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0U ||
      (directory_tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0U ||
      host_standard.Directory != FALSE || host_standard.NumberOfLinks != 1U ||
      directory_standard.Directory == FALSE) {
    return IsolatedWorkerError::service_identity_mismatch;
  }

  WIN32_FILE_ATTRIBUTE_DATA attributes{};
  if (!GetFileAttributesExW(worker_path.c_str(), GetFileExInfoStandard,
                            &attributes)) {
    const DWORD error = GetLastError();
    return error == ERROR_FILE_NOT_FOUND || error == ERROR_PATH_NOT_FOUND
               ? IsolatedWorkerError::service_unavailable
               : IsolatedWorkerError::service_identity_mismatch;
  }
  if ((attributes.dwFileAttributes &
       (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT)) != 0U) {
    return IsolatedWorkerError::service_identity_mismatch;
  }

  DWORD binary_type = 0;
  if (!GetBinaryTypeW(worker_path.c_str(), &binary_type) ||
      binary_type != SCS_64BIT_BINARY) {
    return IsolatedWorkerError::service_identity_mismatch;
  }

  result.file.reset(CreateFileW(
      worker_path.c_str(), FILE_READ_ATTRIBUTES | READ_CONTROL,
      FILE_SHARE_READ, nullptr, OPEN_EXISTING,
      FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT, nullptr));
  FILE_STANDARD_INFO worker_standard{};
  if (!result.file || !query_file_identity(result.file.get(), result.identity) ||
      !query_file_standard_information(result.file.get(), worker_standard) ||
      worker_standard.Directory != FALSE || worker_standard.NumberOfLinks != 1U ||
      result.identity.VolumeSerialNumber !=
          result.host_identity.VolumeSerialNumber ||
      !final_path_for_handle(result.file.get(), result.path)) {
    return IsolatedWorkerError::service_identity_mismatch;
  }

  FILE_ATTRIBUTE_TAG_INFO tag{};
  if (!GetFileInformationByHandleEx(result.file.get(), FileAttributeTagInfo,
                                    &tag, sizeof(tag)) ||
      (tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0U) {
    return IsolatedWorkerError::service_identity_mismatch;
  }
  return IsolatedWorkerError::none;
}

[[nodiscard]] bool process_image_matches(
    const HANDLE process, const FILE_ID_INFO& expected_identity) {
  std::wstring image_path(32'768U, L'\0');
  DWORD characters = static_cast<DWORD>(image_path.size());
  if (!QueryFullProcessImageNameW(process, 0, image_path.data(), &characters) ||
      characters == 0 ||
      static_cast<std::size_t>(characters) >= image_path.size()) {
    return false;
  }
  image_path.resize(characters);
  UniqueHandle image{CreateFileW(
      image_path.c_str(), FILE_READ_ATTRIBUTES | READ_CONTROL, FILE_SHARE_READ,
      nullptr, OPEN_EXISTING,
      FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT, nullptr)};
  FILE_ID_INFO actual{};
  return image && query_file_identity(image.get(), actual) &&
         same_file_identity(expected_identity, actual);
}

[[nodiscard]] bool trusted_path_still_matches(
    const TrustedExecutable& executable) noexcept {
  UniqueHandle candidate{CreateFileW(
      executable.path.c_str(), FILE_READ_ATTRIBUTES | READ_CONTROL,
      FILE_SHARE_READ, nullptr, OPEN_EXISTING,
      FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT, nullptr)};
  FILE_ID_INFO identity{};
  FILE_ATTRIBUTE_TAG_INFO tag{};
  return candidate && query_file_identity(candidate.get(), identity) &&
         same_file_identity(executable.identity, identity) &&
         GetFileInformationByHandleEx(candidate.get(), FileAttributeTagInfo,
                                      &tag, sizeof(tag)) != FALSE &&
         (tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) == 0U;
}

[[nodiscard]] bool process_machine_is_native_x64(
    const HANDLE process) noexcept {
  USHORT process_machine = IMAGE_FILE_MACHINE_UNKNOWN;
  USHORT native_machine = IMAGE_FILE_MACHINE_UNKNOWN;
  return IsWow64Process2(process, &process_machine, &native_machine) != FALSE &&
         process_machine == IMAGE_FILE_MACHINE_UNKNOWN &&
         native_machine == IMAGE_FILE_MACHINE_AMD64;
}

[[nodiscard]] bool current_user_sid(std::vector<std::byte>& token_storage,
                                    PSID& sid) {
  UniqueHandle token;
  HANDLE raw_token = nullptr;
  if (!OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw_token)) {
    return false;
  }
  token.reset(raw_token);

  DWORD bytes = 0;
  static_cast<void>(GetTokenInformation(token.get(), TokenUser, nullptr, 0,
                                        &bytes));
  if (GetLastError() != ERROR_INSUFFICIENT_BUFFER || bytes == 0) {
    return false;
  }
  token_storage.resize(bytes);
  if (!GetTokenInformation(token.get(), TokenUser, token_storage.data(), bytes,
                           &bytes)) {
    return false;
  }
  sid = reinterpret_cast<TOKEN_USER*>(token_storage.data())->User.Sid;
  return IsValidSid(sid) != FALSE;
}

[[nodiscard]] IsolatedWorkerError create_private_pipe(PipePair& pair) {
  std::vector<std::byte> token_storage;
  PSID user_sid = nullptr;
  if (!current_user_sid(token_storage, user_sid)) {
    return IsolatedWorkerError::channel_creation_failed;
  }

  const DWORD acl_bytes = static_cast<DWORD>(
      sizeof(ACL) + sizeof(ACCESS_ALLOWED_ACE) + GetLengthSid(user_sid));
  std::vector<std::byte> acl_storage(acl_bytes);
  auto* acl = reinterpret_cast<ACL*>(acl_storage.data());
  if (!InitializeAcl(acl, acl_bytes, ACL_REVISION) ||
      !AddAccessAllowedAceEx(acl, ACL_REVISION, 0, GENERIC_ALL, user_sid)) {
    return IsolatedWorkerError::channel_creation_failed;
  }

  SECURITY_DESCRIPTOR descriptor{};
  if (!InitializeSecurityDescriptor(&descriptor,
                                    SECURITY_DESCRIPTOR_REVISION) ||
      !SetSecurityDescriptorDacl(&descriptor, TRUE, acl, FALSE)) {
    return IsolatedWorkerError::channel_creation_failed;
  }
  SECURITY_ATTRIBUTES server_security{
      static_cast<DWORD>(sizeof(server_security)), &descriptor, FALSE};

  const std::wstring suffix = random_suffix();
  if (suffix.empty()) {
    return IsolatedWorkerError::resource_exhausted;
  }
  const std::wstring pipe_name =
      L"\\\\.\\pipe\\LOCAL\\OpenHalfLife.IsolatedWorker." + suffix;
  pair.server.reset(CreateNamedPipeW(
      pipe_name.c_str(), PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED |
                             FILE_FLAG_FIRST_PIPE_INSTANCE,
      PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT |
          PIPE_REJECT_REMOTE_CLIENTS,
      1, static_cast<DWORD>(windows::kPipeBufferBytes),
      static_cast<DWORD>(windows::kPipeBufferBytes), 0, &server_security));
  if (!pair.server) {
    return IsolatedWorkerError::channel_creation_failed;
  }

  UniqueHandle connect_event{CreateEventW(nullptr, TRUE, FALSE, nullptr)};
  if (!connect_event) {
    return IsolatedWorkerError::channel_creation_failed;
  }
  OVERLAPPED connect{};
  connect.hEvent = connect_event.get();
#ifdef OHL_WINDOWS_ISOLATED_WORKER_TESTING
  record_launch_stage(windows::IsolatedWorkerLaunchStage::pipe_connect_pending);
#endif
  const BOOL connected = ConnectNamedPipe(pair.server.get(), &connect);
  const DWORD connect_error = connected ? ERROR_SUCCESS : GetLastError();
  if (!connected && connect_error != ERROR_IO_PENDING &&
      connect_error != ERROR_PIPE_CONNECTED) {
    return IsolatedWorkerError::channel_creation_failed;
  }
  // No client has received the random pipe name yet. A preconnected peer is
  // therefore an integrity failure rather than a successful connection.
  if (!connected && connect_error == ERROR_PIPE_CONNECTED) {
    return IsolatedWorkerError::channel_creation_failed;
  }
  PendingOverlappedGuard pending_connect{
      pair.server.get(), connect, connect_error == ERROR_IO_PENDING};

  SECURITY_ATTRIBUTES child_security{
      static_cast<DWORD>(sizeof(child_security)), nullptr, FALSE};
  pair.child.reset(CreateFileW(
      pipe_name.c_str(), GENERIC_READ | GENERIC_WRITE, 0, &child_security,
      OPEN_EXISTING,
      FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_ANONYMOUS,
      nullptr));
  if (!pair.child) {
    return IsolatedWorkerError::channel_creation_failed;
  }

  if (!pending_connect.complete()) {
    return IsolatedWorkerError::channel_creation_failed;
  }
#ifdef OHL_WINDOWS_ISOLATED_WORKER_TESTING
  record_launch_stage(windows::IsolatedWorkerLaunchStage::pipe_connected);
#endif

  DWORD flags = 0;
  if (!GetHandleInformation(pair.child.get(), &flags) ||
      (flags & HANDLE_FLAG_INHERIT) != 0U) {
    return IsolatedWorkerError::channel_creation_failed;
  }
  return IsolatedWorkerError::none;
}

[[nodiscard]] bool configure_job(const HANDLE job) noexcept {
  JOBOBJECT_EXTENDED_LIMIT_INFORMATION limits{};
  limits.BasicLimitInformation.LimitFlags =
      JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_ACTIVE_PROCESS |
      JOB_OBJECT_LIMIT_PROCESS_MEMORY | JOB_OBJECT_LIMIT_JOB_MEMORY |
      JOB_OBJECT_LIMIT_PROCESS_TIME | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
  limits.BasicLimitInformation.ActiveProcessLimit = 1;
  limits.BasicLimitInformation.PerProcessUserTimeLimit.QuadPart =
      windows::kProcessCpuTimeLimit100ns;
  limits.ProcessMemoryLimit = windows::kProcessMemoryLimitBytes;
  limits.JobMemoryLimit = windows::kJobMemoryLimitBytes;
  if (!SetInformationJobObject(job, JobObjectExtendedLimitInformation, &limits,
                               sizeof(limits))) {
    return false;
  }

  JOBOBJECT_CPU_RATE_CONTROL_INFORMATION cpu{};
  cpu.ControlFlags = JOB_OBJECT_CPU_RATE_CONTROL_ENABLE |
                     JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP;
  cpu.CpuRate = windows::kCpuHardCapHundredthsOfPercent;
  if (!SetInformationJobObject(job, JobObjectCpuRateControlInformation, &cpu,
                               sizeof(cpu))) {
    return false;
  }

  JOBOBJECT_BASIC_UI_RESTRICTIONS ui{};
  ui.UIRestrictionsClass =
      JOB_OBJECT_UILIMIT_DESKTOP | JOB_OBJECT_UILIMIT_DISPLAYSETTINGS |
      JOB_OBJECT_UILIMIT_EXITWINDOWS | JOB_OBJECT_UILIMIT_GLOBALATOMS |
      JOB_OBJECT_UILIMIT_HANDLES | JOB_OBJECT_UILIMIT_READCLIPBOARD |
      JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS | JOB_OBJECT_UILIMIT_WRITECLIPBOARD;
  return SetInformationJobObject(job, JobObjectBasicUIRestrictions, &ui,
                                 sizeof(ui)) != FALSE;
}

[[nodiscard]] bool verify_job(const HANDLE job, const HANDLE process) noexcept {
  BOOL in_job = FALSE;
  if (!IsProcessInJob(process, job, &in_job) || !in_job) {
    return false;
  }

  JOBOBJECT_EXTENDED_LIMIT_INFORMATION limits{};
  if (!QueryInformationJobObject(job, JobObjectExtendedLimitInformation,
                                 &limits, sizeof(limits), nullptr)) {
    return false;
  }
  constexpr DWORD required_limits =
      JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_ACTIVE_PROCESS |
      JOB_OBJECT_LIMIT_PROCESS_MEMORY | JOB_OBJECT_LIMIT_JOB_MEMORY |
      JOB_OBJECT_LIMIT_PROCESS_TIME | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
  if ((limits.BasicLimitInformation.LimitFlags & required_limits) !=
          required_limits ||
      limits.BasicLimitInformation.ActiveProcessLimit != 1 ||
      limits.BasicLimitInformation.PerProcessUserTimeLimit.QuadPart !=
          windows::kProcessCpuTimeLimit100ns ||
      limits.ProcessMemoryLimit !=
          static_cast<SIZE_T>(windows::kProcessMemoryLimitBytes) ||
      limits.JobMemoryLimit !=
          static_cast<SIZE_T>(windows::kJobMemoryLimitBytes)) {
    return false;
  }

  JOBOBJECT_CPU_RATE_CONTROL_INFORMATION cpu{};
  if (!QueryInformationJobObject(job, JobObjectCpuRateControlInformation, &cpu,
                                 sizeof(cpu), nullptr)) {
    return false;
  }
  constexpr DWORD required_cpu = JOB_OBJECT_CPU_RATE_CONTROL_ENABLE |
                                 JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP;
  if ((cpu.ControlFlags & required_cpu) != required_cpu ||
      cpu.CpuRate != windows::kCpuHardCapHundredthsOfPercent) {
    return false;
  }

  JOBOBJECT_BASIC_UI_RESTRICTIONS ui{};
  if (!QueryInformationJobObject(job, JobObjectBasicUIRestrictions, &ui,
                                 sizeof(ui), nullptr)) {
    return false;
  }
  constexpr DWORD required_ui =
      JOB_OBJECT_UILIMIT_DESKTOP | JOB_OBJECT_UILIMIT_DISPLAYSETTINGS |
      JOB_OBJECT_UILIMIT_EXITWINDOWS | JOB_OBJECT_UILIMIT_GLOBALATOMS |
      JOB_OBJECT_UILIMIT_HANDLES | JOB_OBJECT_UILIMIT_READCLIPBOARD |
      JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS | JOB_OBJECT_UILIMIT_WRITECLIPBOARD;
  return (ui.UIRestrictionsClass & required_ui) == required_ui;
}

[[nodiscard]] bool query_token_information(const HANDLE token,
                                           const TOKEN_INFORMATION_CLASS kind,
                                           std::vector<std::byte>& storage) {
  DWORD bytes = 0;
  static_cast<void>(GetTokenInformation(token, kind, nullptr, 0, &bytes));
  if (GetLastError() != ERROR_INSUFFICIENT_BUFFER || bytes == 0) {
    return false;
  }
  storage.resize(bytes);
  return GetTokenInformation(token, kind, storage.data(), bytes, &bytes) != FALSE;
}

// TokenIsLessPrivilegedAppContainer is not reliably queryable from user
// mode; hosted Windows Server runners reject it for genuine LPAC children.
// The kernel marks LPAC tokens with the WIN://NOALLAPPPKG security
// attribute, so require that attribute with a nonzero integer value.
[[nodiscard]] bool token_has_lpac_attribute(const HANDLE token) {
  std::vector<std::byte> storage;
  if (!query_token_information(token, TokenSecurityAttributes, storage)) {
    return false;
  }
  return windows::token_security_attributes_mark_lpac(storage.data());
}

[[nodiscard]] bool verify_lpac_token(const HANDLE process,
                                     const PSID expected_sid) {
  HANDLE raw_token = nullptr;
#ifdef OHL_WINDOWS_ISOLATED_WORKER_TESTING
  record_launch_stage(windows::IsolatedWorkerLaunchStage::token_open_pending);
#endif
  if (!OpenProcessToken(process, TOKEN_QUERY, &raw_token)) {
    return false;
  }
  UniqueHandle token{raw_token};
#ifdef OHL_WINDOWS_ISOLATED_WORKER_TESTING
  record_launch_stage(windows::IsolatedWorkerLaunchStage::token_opened);
  record_launch_stage(
      windows::IsolatedWorkerLaunchStage::app_container_verification_pending);
#endif

  DWORD is_app_container = 0;
  DWORD returned = 0;
  if (!GetTokenInformation(token.get(), TokenIsAppContainer,
                           &is_app_container, sizeof(is_app_container),
                           &returned) ||
      is_app_container == 0) {
    return false;
  }
#ifdef OHL_WINDOWS_ISOLATED_WORKER_TESTING
  record_launch_stage(
      windows::IsolatedWorkerLaunchStage::app_container_verified);
  record_launch_stage(
      windows::IsolatedWorkerLaunchStage::lpac_verification_pending);
#endif

  if (!token_has_lpac_attribute(token.get())) {
    return false;
  }
  // WIN://NOALLAPPPKG is the kernel's LPAC marker. LPAC causes access checks
  // to disregard ALL_APPLICATION_PACKAGES; it does not require that SID to be
  // absent from the token's general group list.
#ifdef OHL_WINDOWS_ISOLATED_WORKER_TESTING
  record_launch_stage(windows::IsolatedWorkerLaunchStage::lpac_verified);
  record_launch_stage(
      windows::IsolatedWorkerLaunchStage::app_sid_verification_pending);
#endif

  std::vector<std::byte> app_sid_storage;
  if (!query_token_information(token.get(), TokenAppContainerSid,
                               app_sid_storage)) {
    return false;
  }
  const auto* app_sid =
      reinterpret_cast<const TOKEN_APPCONTAINER_INFORMATION*>(
          app_sid_storage.data());
  if (app_sid->TokenAppContainer == nullptr ||
      !EqualSid(app_sid->TokenAppContainer, expected_sid)) {
    return false;
  }
#ifdef OHL_WINDOWS_ISOLATED_WORKER_TESTING
  record_launch_stage(
      windows::IsolatedWorkerLaunchStage::app_sid_verified);
  record_launch_stage(
      windows::IsolatedWorkerLaunchStage::capability_verification_pending);
#endif

  std::vector<std::byte> capability_storage;
  if (!query_token_information(token.get(), TokenCapabilities,
                               capability_storage)) {
    return false;
  }
  const auto* capabilities =
      reinterpret_cast<const TOKEN_GROUPS*>(capability_storage.data());
  if (capabilities->GroupCount != 0) {
    return false;
  }
#ifdef OHL_WINDOWS_ISOLATED_WORKER_TESTING
  record_launch_stage(
      windows::IsolatedWorkerLaunchStage::capabilities_verified);
#endif
  return true;
}

template <typename Policy>
[[nodiscard]] bool get_mitigation(const HANDLE process,
                                  const PROCESS_MITIGATION_POLICY kind,
                                  Policy& policy) noexcept {
  return GetProcessMitigationPolicy(process, kind, &policy, sizeof(policy)) !=
         FALSE;
}

[[nodiscard]] bool verify_mitigations(const HANDLE process) noexcept {
  PROCESS_MITIGATION_DEP_POLICY dep{};
  PROCESS_MITIGATION_ASLR_POLICY aslr{};
  PROCESS_MITIGATION_STRICT_HANDLE_CHECK_POLICY handles{};
  PROCESS_MITIGATION_SYSTEM_CALL_DISABLE_POLICY system_calls{};
  PROCESS_MITIGATION_EXTENSION_POINT_DISABLE_POLICY extensions{};
  PROCESS_MITIGATION_DYNAMIC_CODE_POLICY dynamic_code{};
  PROCESS_MITIGATION_FONT_DISABLE_POLICY fonts{};
  PROCESS_MITIGATION_IMAGE_LOAD_POLICY images{};
  PROCESS_MITIGATION_CHILD_PROCESS_POLICY children{};

  return get_mitigation(process, ProcessDEPPolicy, dep) && dep.Enable != 0 &&
         get_mitigation(process, ProcessASLRPolicy, aslr) &&
         aslr.EnableBottomUpRandomization != 0 &&
         aslr.EnableForceRelocateImages != 0 &&
         aslr.EnableHighEntropy != 0 &&
         get_mitigation(process, ProcessStrictHandleCheckPolicy, handles) &&
         handles.RaiseExceptionOnInvalidHandleReference != 0 &&
         get_mitigation(process, ProcessSystemCallDisablePolicy, system_calls) &&
         system_calls.DisallowWin32kSystemCalls != 0 &&
         get_mitigation(process, ProcessExtensionPointDisablePolicy,
                        extensions) &&
         extensions.DisableExtensionPoints != 0 &&
         get_mitigation(process, ProcessDynamicCodePolicy, dynamic_code) &&
         dynamic_code.ProhibitDynamicCode != 0 &&
         get_mitigation(process, ProcessFontDisablePolicy, fonts) &&
         fonts.DisableNonSystemFonts != 0 &&
         get_mitigation(process, ProcessImageLoadPolicy, images) &&
         images.NoRemoteImages != 0 &&
         images.NoLowMandatoryLabelImages != 0 &&
         images.PreferSystem32Images != 0 &&
         get_mitigation(process, ProcessChildProcessPolicy, children) &&
         children.NoChildProcessCreation != 0;
}

[[nodiscard]] std::wstring parent_environment_value(const wchar_t* const name) {
  std::wstring value(32'768U, L'\0');
  const DWORD length = GetEnvironmentVariableW(
      name, value.data(), static_cast<DWORD>(value.size()));
  if (length == 0 || static_cast<std::size_t>(length) >= value.size()) {
    return {};
  }
  value.resize(length);
  return value;
}

[[nodiscard]] std::vector<wchar_t> minimal_environment() {
  std::wstring windows_directory(32'768U, L'\0');
  const UINT length = GetWindowsDirectoryW(
      windows_directory.data(), static_cast<UINT>(windows_directory.size()));
  if (length == 0 ||
      static_cast<std::size_t>(length) >= windows_directory.size()) {
    return {};
  }
  windows_directory.resize(length);

  // AppContainer activation resolves the package profile paths through the
  // supplied environment block; CreateProcessW fails with
  // ERROR_ENVVAR_NOT_FOUND when the block lacks the profile variables it
  // expects. Forward only this fixed, non-secret path allowlist. A Unicode
  // environment block must stay sorted by name, so SystemRoot is emitted in
  // its alphabetical slot. Absent parent variables are skipped.
  static constexpr const wchar_t* kEnvironmentAllowlist[] = {
      L"ALLUSERSPROFILE", L"APPDATA", L"LOCALAPPDATA",
      L"ProgramData",     L"SystemDrive", L"SystemRoot",
      L"TEMP",            L"TMP",     L"USERPROFILE",
  };
  std::vector<wchar_t> result;
  for (const wchar_t* const name : kEnvironmentAllowlist) {
    const bool is_system_root = std::wstring_view{name} == L"SystemRoot";
    const std::wstring value =
        is_system_root ? windows_directory : parent_environment_value(name);
    if (value.empty()) {
      continue;
    }
    result.insert(result.end(), name, name + std::wcslen(name));
    result.push_back(L'=');
    result.insert(result.end(), value.begin(), value.end());
    result.push_back(L'\0');
  }
  if (result.empty()) {
    return {};
  }
  result.push_back(L'\0');
  return result;
}

[[nodiscard]] std::vector<wchar_t> worker_command_line(
    const TrustedExecutable& executable, const HANDLE child_pipe) {
  std::wstring command;
  command.reserve(executable.path.size() + 160U);
  command.push_back(L'"');
  command.append(executable.path);
  command.append(L"\" ");
  command.append(windows::kMediaParserModeArgument);
  command.push_back(L' ');
  command.append(windows::kIpcHandleArgumentPrefix);
  command.append(std::to_wstring(reinterpret_cast<std::uintptr_t>(child_pipe)));
  command.push_back(L' ');
  command.append(windows::kBootstrapArgument);
  std::vector<wchar_t> result(command.begin(), command.end());
  result.push_back(L'\0');
  return result;
}

[[nodiscard]] DWORD64 required_mitigation_policy() noexcept {
  return PROCESS_CREATION_MITIGATION_POLICY_DEP_ENABLE |
         PROCESS_CREATION_MITIGATION_POLICY_SEHOP_ENABLE |
         PROCESS_CREATION_MITIGATION_POLICY_FORCE_RELOCATE_IMAGES_ALWAYS_ON_REQ_RELOCS |
         PROCESS_CREATION_MITIGATION_POLICY_HEAP_TERMINATE_ALWAYS_ON |
         PROCESS_CREATION_MITIGATION_POLICY_BOTTOM_UP_ASLR_ALWAYS_ON |
         PROCESS_CREATION_MITIGATION_POLICY_HIGH_ENTROPY_ASLR_ALWAYS_ON |
         PROCESS_CREATION_MITIGATION_POLICY_STRICT_HANDLE_CHECKS_ALWAYS_ON |
         PROCESS_CREATION_MITIGATION_POLICY_WIN32K_SYSTEM_CALL_DISABLE_ALWAYS_ON |
         PROCESS_CREATION_MITIGATION_POLICY_EXTENSION_POINT_DISABLE_ALWAYS_ON |
         PROCESS_CREATION_MITIGATION_POLICY_PROHIBIT_DYNAMIC_CODE_ALWAYS_ON |
         PROCESS_CREATION_MITIGATION_POLICY_FONT_DISABLE_ALWAYS_ON |
         PROCESS_CREATION_MITIGATION_POLICY_IMAGE_LOAD_NO_REMOTE_ALWAYS_ON |
         PROCESS_CREATION_MITIGATION_POLICY_IMAGE_LOAD_NO_LOW_LABEL_ALWAYS_ON |
         PROCESS_CREATION_MITIGATION_POLICY_IMAGE_LOAD_PREFER_SYSTEM32_ALWAYS_ON;
}

[[nodiscard]] bool is_resource_exit(const DWORD exit_code) noexcept {
  constexpr DWORD status_no_memory = 0xC0000017U;
  constexpr DWORD status_quota_exceeded = 0xC0000044U;
  constexpr DWORD status_working_set_quota = 0xC00000A1U;
  constexpr DWORD status_commitment_limit = 0xC000012DU;
  return exit_code == status_no_memory || exit_code == status_quota_exceeded ||
         exit_code == status_working_set_quota ||
         exit_code == status_commitment_limit;
}

class WindowsIsolatedWorkerBackend final : public IsolatedWorkerBackend {
 public:
  WindowsIsolatedWorkerBackend() noexcept = default;

  void adopt(UniqueHandle process, UniqueHandle job, UniqueHandle pipe,
             std::unique_ptr<ProfileRecord> profile) noexcept {
    process_ = std::move(process);
    job_ = std::move(job);
    pipe_ = std::move(pipe);
    profile_ = std::move(profile);
  }

  ~WindowsIsolatedWorkerBackend() override {
    abort_io();
    close_channel();
    request_termination();

    // Closing this private kill-on-close job is the final fail-closed backstop
    // if TerminateJobObject itself was unavailable or failed.
    job_.reset();
    if (process_) {
      static_cast<void>(WaitForSingleObject(process_.get(), INFINITE));
    }
    // Object destruction concurrent with a member call is outside the public
    // lifetime contract, so no channel lease can remain here.
    pipe_.reset();
    process_.reset();
    if (!cleanup_profile()) {
      PendingProfileRegistry::instance().retain(std::move(profile_));
    }
  }

  [[nodiscard]] IsolatedWorkerIoResult read_exact(
      const std::span<std::byte> destination,
      const Clock::time_point deadline,
      const IsolatedWorkerCancellationToken cancellation) noexcept override {
    return transfer(destination.data(), destination.size(), false, deadline,
                    cancellation);
  }

  [[nodiscard]] IsolatedWorkerIoResult write_all(
      const std::span<const std::byte> source,
      const Clock::time_point deadline,
      const IsolatedWorkerCancellationToken cancellation) noexcept override {
    return transfer(const_cast<std::byte*>(source.data()), source.size(), true,
                    deadline, cancellation);
  }

  void abort_io() noexcept override {
    aborted_.store(true);
    cancel_all_io();
  }

  void close_channel() noexcept override {
    closed_.store(true);
    HANDLE pipe_to_close = nullptr;
    AcquireSRWLockExclusive(&pipe_lock_);
    if (pipe_) {
      if (active_io_ == 0) {
        pipe_to_close = pipe_.release();
      } else {
        const BOOL cancelled = CancelIoEx(pipe_.get(), nullptr);
        if (!cancelled && GetLastError() != ERROR_NOT_FOUND) {
          io_cancel_failed_.store(true);
        }
      }
    }
    ReleaseSRWLockExclusive(&pipe_lock_);

    if (pipe_to_close != nullptr) {
      CloseHandle(pipe_to_close);
    }
  }

  void request_termination() noexcept override {
    bool expected = false;
    if (!termination_requested_.compare_exchange_strong(expected, true)) {
      return;
    }
    if (!job_ || !TerminateJobObject(job_.get(), windows::kTerminationExitCode)) {
      termination_failed_.store(true);
    }
  }

  [[nodiscard]] IsolatedWorkerWaitResult wait(
      const Clock::time_point deadline) noexcept override {
    if (!process_) {
      return {.exit = IsolatedWorkerExitKind::unknown,
              .error = IsolatedWorkerError::reap_failed};
    }
    const DWORD wait_result =
        WaitForSingleObject(process_.get(), wait_milliseconds(deadline));
    if (wait_result == WAIT_TIMEOUT) {
      return {.exit = IsolatedWorkerExitKind::running,
              .error = IsolatedWorkerError::timeout};
    }
    if (wait_result != WAIT_OBJECT_0) {
      request_termination();
      return {.exit = IsolatedWorkerExitKind::unknown,
              .error = IsolatedWorkerError::reap_failed};
    }

    DWORD exit_code = 0;
    if (!GetExitCodeProcess(process_.get(), &exit_code) ||
        exit_code == STILL_ACTIVE) {
      request_termination();
      return {.exit = IsolatedWorkerExitKind::unknown,
              .error = IsolatedWorkerError::reap_failed};
    }
    IsolatedWorkerWaitResult result;
    if (termination_requested_.load()) {
      result.exit = IsolatedWorkerExitKind::terminated;
    } else if (exit_code == 0) {
      result.exit = IsolatedWorkerExitKind::clean;
    } else if (is_resource_exit(exit_code)) {
      result.exit = IsolatedWorkerExitKind::resource_limit;
    } else {
      result.exit = (exit_code & 0x80000000U) != 0U
                        ? IsolatedWorkerExitKind::crashed
                        : IsolatedWorkerExitKind::failed;
    }
    if (!cleanup_profile()) {
      result.error = IsolatedWorkerError::confinement_unavailable;
    }
    return result;
  }

  [[nodiscard]] IsolatedWorkerWaitResult terminate_and_wait(
      const Clock::time_point deadline) noexcept override {
    request_termination();
    const IsolatedWorkerWaitResult result = wait(deadline);
    if (termination_failed_.load()) {
      return {.exit = IsolatedWorkerExitKind::unknown,
              .error = IsolatedWorkerError::termination_failed};
    }
    return result;
  }

 private:
  class PipeLease final {
   public:
    explicit PipeLease(WindowsIsolatedWorkerBackend& owner) noexcept
        : owner_(&owner), pipe_(owner.acquire_pipe()) {}
    ~PipeLease() {
      if (pipe_ != nullptr) {
        owner_->release_pipe();
      }
    }

    PipeLease(const PipeLease&) = delete;
    PipeLease& operator=(const PipeLease&) = delete;

    [[nodiscard]] HANDLE get() const noexcept { return pipe_; }
    [[nodiscard]] explicit operator bool() const noexcept {
      return pipe_ != nullptr;
    }

   private:
    WindowsIsolatedWorkerBackend* owner_;
    HANDLE pipe_;
  };

  void cancel_all_io() noexcept {
    AcquireSRWLockShared(&pipe_lock_);
    if (pipe_) {
      const BOOL cancelled = CancelIoEx(pipe_.get(), nullptr);
      if (!cancelled && GetLastError() != ERROR_NOT_FOUND) {
        io_cancel_failed_.store(true);
      }
    }
    ReleaseSRWLockShared(&pipe_lock_);
  }

  [[nodiscard]] HANDLE acquire_pipe() noexcept {
    AcquireSRWLockExclusive(&pipe_lock_);
    HANDLE result = nullptr;
    if (pipe_ && !closed_.load()) {
      ++active_io_;
      result = pipe_.get();
    }
    ReleaseSRWLockExclusive(&pipe_lock_);
    return result;
  }

  void release_pipe() noexcept {
    HANDLE pipe_to_close = nullptr;
    AcquireSRWLockExclusive(&pipe_lock_);
    if (active_io_ > 0) {
      --active_io_;
    }
    if (active_io_ == 0 && closed_.load() && pipe_) {
      pipe_to_close = pipe_.release();
    }
    ReleaseSRWLockExclusive(&pipe_lock_);
    if (pipe_to_close != nullptr) {
      CloseHandle(pipe_to_close);
    }
  }

  [[nodiscard]] IsolatedWorkerIoResult transfer(
      std::byte* const buffer, const std::size_t size, const bool write,
      const Clock::time_point deadline,
      const IsolatedWorkerCancellationToken cancellation) noexcept {
    if (aborted_.load() || cancellation.cancellation_requested()) {
      return {.error = IsolatedWorkerError::cancelled};
    }
    if (closed_.load()) {
      return {.error = IsolatedWorkerError::invalid_state};
    }
    PipeLease pipe{*this};
    if (!pipe) {
      return {.error = IsolatedWorkerError::invalid_state};
    }

    UniqueHandle event{CreateEventW(nullptr, TRUE, FALSE, nullptr)};
    if (!event) {
      return {.error = IsolatedWorkerError::resource_exhausted};
    }

    std::size_t total = 0;
    while (total < size) {
      if (aborted_.load() || closed_.load() ||
          cancellation.cancellation_requested()) {
        return {.bytes_transferred = total,
                .error = IsolatedWorkerError::cancelled};
      }
      if (deadline <= Clock::now()) {
        return {.bytes_transferred = total,
                .error = IsolatedWorkerError::timeout};
      }

      if (!ResetEvent(event.get())) {
        return {.bytes_transferred = total,
                .error = IsolatedWorkerError::io_failure};
      }
      OVERLAPPED operation{};
      operation.hEvent = event.get();
      const DWORD request = static_cast<DWORD>(std::min<std::size_t>(
          size - total, std::numeric_limits<DWORD>::max()));
      DWORD transferred = 0;
      const BOOL completed =
          write ? WriteFile(pipe.get(), buffer + total, request, &transferred,
                            &operation)
                : ReadFile(pipe.get(), buffer + total, request, &transferred,
                           &operation);
      if (!completed) {
        const DWORD error = GetLastError();
        if (error != ERROR_IO_PENDING) {
          if (error == ERROR_BROKEN_PIPE || error == ERROR_PIPE_NOT_CONNECTED ||
              error == ERROR_NO_DATA) {
            return {.bytes_transferred = total,
                    .error = IsolatedWorkerError::peer_closed};
          }
          if (error == ERROR_OPERATION_ABORTED) {
            return {.bytes_transferred = total,
                    .error = IsolatedWorkerError::cancelled};
          }
          return {.bytes_transferred = total,
                  .error = IsolatedWorkerError::io_failure};
        }

        const std::array<HANDLE, 2> waits{event.get(), process_.get()};
        bool pending = true;
        while (pending) {
          if (aborted_.load() || closed_.load() ||
              cancellation.cancellation_requested()) {
            cancel_and_drain(pipe.get(), operation);
            return {.bytes_transferred = total,
                    .error = IsolatedWorkerError::cancelled};
          }

          const DWORD deadline_wait = wait_milliseconds(deadline);
          constexpr DWORD cancellation_poll = static_cast<DWORD>(
              windows::kCancellationObservationMilliseconds);
          const DWORD operation_wait =
              deadline_wait == INFINITE
                  ? cancellation_poll
                  : std::min(deadline_wait, cancellation_poll);
          const DWORD wait_result = WaitForMultipleObjects(
              static_cast<DWORD>(waits.size()), waits.data(), FALSE,
              operation_wait);

          if (wait_result == WAIT_OBJECT_0) {
            if (GetOverlappedResult(pipe.get(), &operation, &transferred,
                                    FALSE)) {
              pending = false;
              continue;
            }
            const DWORD completion_error = GetLastError();
            if (completion_error == ERROR_IO_INCOMPLETE) {
              continue;
            }
            if (completion_error == ERROR_OPERATION_ABORTED) {
              return {.bytes_transferred = total,
                      .error = IsolatedWorkerError::cancelled};
            }
            if (completion_error == ERROR_BROKEN_PIPE ||
                completion_error == ERROR_PIPE_NOT_CONNECTED || completion_error == ERROR_NO_DATA) {
              return {.bytes_transferred = total,
                      .error = IsolatedWorkerError::peer_closed};
            }
            return {.bytes_transferred = total,
                    .error = IsolatedWorkerError::io_failure};
          }

          DWORD raced_bytes = 0;
          if (GetOverlappedResult(pipe.get(), &operation, &raced_bytes,
                                  FALSE)) {
            transferred = raced_bytes;
            pending = false;
            continue;
          }
          const DWORD raced_error = GetLastError();
          if (raced_error != ERROR_IO_INCOMPLETE) {
            if (raced_error == ERROR_OPERATION_ABORTED) {
              return {.bytes_transferred = total,
                      .error = IsolatedWorkerError::cancelled};
            }
            if (raced_error == ERROR_BROKEN_PIPE ||
                raced_error == ERROR_PIPE_NOT_CONNECTED ||
                raced_error == ERROR_NO_DATA) {
              return {.bytes_transferred = total,
                      .error = IsolatedWorkerError::peer_closed};
            }
            return {.bytes_transferred = total,
                    .error = IsolatedWorkerError::io_failure};
          }

          if (cancellation.cancellation_requested() || aborted_.load() ||
              closed_.load()) {
            cancel_and_drain(pipe.get(), operation);
            return {.bytes_transferred = total,
                    .error = IsolatedWorkerError::cancelled};
          }
          if (deadline <= Clock::now()) {
            cancel_and_drain(pipe.get(), operation);
            return {.bytes_transferred = total,
                    .error = IsolatedWorkerError::timeout};
          }
          if (wait_result == WAIT_OBJECT_0 + 1U) {
            cancel_and_drain(pipe.get(), operation);
            return {.bytes_transferred = total,
                    .error = IsolatedWorkerError::peer_closed};
          }
          if (wait_result != WAIT_TIMEOUT) {
            cancel_and_drain(pipe.get(), operation);
            return {.bytes_transferred = total,
                    .error = IsolatedWorkerError::io_failure};
          }
        }
      }

      if (transferred == 0) {
        return {.bytes_transferred = total,
                .error = IsolatedWorkerError::peer_closed};
      }
      total += transferred;
    }

    if (io_cancel_failed_.load()) {
      return {.bytes_transferred = total,
              .error = IsolatedWorkerError::io_failure};
    }
    return {.bytes_transferred = total, .error = IsolatedWorkerError::none};
  }

  static void cancel_and_drain(const HANDLE pipe,
                               OVERLAPPED& operation) noexcept {
    static_cast<void>(CancelIoEx(pipe, &operation));
    DWORD ignored = 0;
    // CancelIoEx is only a request. The stack OVERLAPPED and its event remain
    // alive until the operation reaches a terminal state.
    static_cast<void>(
        GetOverlappedResult(pipe, &operation, &ignored, TRUE));
  }

  [[nodiscard]] bool cleanup_profile() noexcept {
    AcquireSRWLockExclusive(&profile_lock_);
    bool cleaned = true;
    if (profile_ != nullptr &&
        !delete_profile_once(profile_->name) &&
        !delete_profile_once(profile_->name)) {
      cleaned = false;
    } else if (profile_ != nullptr) {
      profile_.reset();
    }
    ReleaseSRWLockExclusive(&profile_lock_);
    return cleaned;
  }

  UniqueHandle process_;
  UniqueHandle job_;
  UniqueHandle pipe_;
  std::unique_ptr<ProfileRecord> profile_;
  SRWLOCK profile_lock_ = SRWLOCK_INIT;
  SRWLOCK pipe_lock_ = SRWLOCK_INIT;
  unsigned int active_io_{0};
  std::atomic_bool aborted_{false};
  std::atomic_bool closed_{false};
  std::atomic_bool io_cancel_failed_{false};
  std::atomic_bool termination_requested_{false};
  std::atomic_bool termination_failed_{false};
};

}  // namespace

#ifdef OHL_WINDOWS_ISOLATED_WORKER_TESTING
namespace windows {
IsolatedWorkerLaunchStage last_isolated_worker_launch_stage() noexcept {
  return g_last_launch_stage.load();
}

IsolatedWorkerCreateProcessFailure
last_isolated_worker_create_process_failure() noexcept {
  return g_last_create_process_failure.load();
}

std::uint32_t last_isolated_worker_create_process_error() noexcept {
  return static_cast<std::uint32_t>(g_last_create_process_error.load());
}
}  // namespace windows
#endif
IsolatedWorkerBackendLaunchResult launch_isolated_worker_backend(
    const IsolatedWorkerService service,
    const Clock::time_point startup_deadline) noexcept {
  try {
#ifdef OHL_WINDOWS_ISOLATED_WORKER_TESTING
    record_launch_stage(windows::IsolatedWorkerLaunchStage::launch_started);
    g_last_create_process_failure.store(
        windows::IsolatedWorkerCreateProcessFailure::none);
#endif
    if (startup_deadline <= Clock::now()) {
      return {.backend = nullptr, .error = IsolatedWorkerError::timeout};
    }
    if (!PendingProfileRegistry::instance().retry_all()) {
      return {.backend = nullptr,
              .error = IsolatedWorkerError::confinement_unavailable};
    }

    TrustedExecutable executable;
#ifdef OHL_WINDOWS_ISOLATED_WORKER_TESTING
    record_launch_stage(
        windows::IsolatedWorkerLaunchStage::worker_identity_pending);
#endif
    const IsolatedWorkerError executable_error =
        locate_trusted_executable(service, executable);
    if (executable_error != IsolatedWorkerError::none) {
      return {.backend = nullptr, .error = executable_error};
    }
#ifdef OHL_WINDOWS_ISOLATED_WORKER_TESTING
    record_launch_stage(
        windows::IsolatedWorkerLaunchStage::worker_identity_verified);
#endif

    const std::wstring suffix = random_suffix();
    if (suffix.empty()) {
      return {.backend = nullptr,
              .error = IsolatedWorkerError::resource_exhausted};
    }
    const std::wstring profile_name = L"OpenHalfLife.IsolatedWorker." + suffix;
    ProfileGuard profile_guard{
        std::make_unique<ProfileRecord>(profile_name)};
    PSID raw_app_container_sid = nullptr;
#ifdef OHL_WINDOWS_ISOLATED_WORKER_TESTING
    record_launch_stage(
        windows::IsolatedWorkerLaunchStage::profile_creation_pending);
#endif
    const HRESULT profile_result = CreateAppContainerProfile(
        profile_name.c_str(), L"Open Half-Life isolated worker",
        L"Ephemeral zero-capability worker profile", nullptr, 0,
        &raw_app_container_sid);
    if (FAILED(profile_result) || raw_app_container_sid == nullptr) {
      return {.backend = nullptr,
              .error = IsolatedWorkerError::confinement_unavailable};
    }
    profile_guard.arm();
#ifdef OHL_WINDOWS_ISOLATED_WORKER_TESTING
    record_launch_stage(windows::IsolatedWorkerLaunchStage::profile_created);
#endif
    UniqueSid app_container_sid{raw_app_container_sid};

    IsolatedWorkerBackendLaunchResult result =
        [&]() -> IsolatedWorkerBackendLaunchResult {
      PipePair pipe;
#ifdef OHL_WINDOWS_ISOLATED_WORKER_TESTING
      record_launch_stage(
          windows::IsolatedWorkerLaunchStage::pipe_creation_pending);
#endif
      const IsolatedWorkerError pipe_error = create_private_pipe(pipe);
      if (pipe_error != IsolatedWorkerError::none) {
        return {.backend = nullptr, .error = pipe_error};
      }
#ifdef OHL_WINDOWS_ISOLATED_WORKER_TESTING
      record_launch_stage(windows::IsolatedWorkerLaunchStage::pipe_created);
#endif

#ifdef OHL_WINDOWS_ISOLATED_WORKER_TESTING
      record_launch_stage(
          windows::IsolatedWorkerLaunchStage::job_configuration_pending);
#endif
      UniqueHandle job{CreateJobObjectW(nullptr, nullptr)};
      if (!job || !configure_job(job.get())) {
        return {.backend = nullptr,
                .error = IsolatedWorkerError::confinement_unavailable};
      }
#ifdef OHL_WINDOWS_ISOLATED_WORKER_TESTING
      record_launch_stage(windows::IsolatedWorkerLaunchStage::job_configured);
#endif

      std::vector<wchar_t> environment = minimal_environment();
      std::vector<wchar_t> command_line =
          worker_command_line(executable, pipe.child.get());
      if (environment.empty() || command_line.empty()) {
        return {.backend = nullptr,
                .error = IsolatedWorkerError::resource_exhausted};
      }

      SECURITY_CAPABILITIES security_capabilities{};
      security_capabilities.AppContainerSid = app_container_sid.get();
      security_capabilities.Capabilities = nullptr;
      security_capabilities.CapabilityCount = 0;
      security_capabilities.Reserved = 0;
      DWORD all_application_packages =
          PROCESS_CREATION_ALL_APPLICATION_PACKAGES_OPT_OUT;
      DWORD64 mitigations = required_mitigation_policy();
      DWORD child_process_policy = PROCESS_CREATION_CHILD_PROCESS_RESTRICTED;
      std::array<HANDLE, 1> inherited_handles{pipe.child.get()};
      std::array<HANDLE, 1> jobs{job.get()};

#ifdef OHL_WINDOWS_ISOLATED_WORKER_TESTING
      record_launch_stage(
          windows::IsolatedWorkerLaunchStage::process_policy_pending);
#endif
      AttributeList attributes;
      if (!attributes.initialize(6) ||
          !attributes.set(PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
                          security_capabilities) ||
          !attributes.set(PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY,
                          all_application_packages) ||
          !attributes.set_span(PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
                               std::span{inherited_handles}) ||
          !attributes.set(PROC_THREAD_ATTRIBUTE_MITIGATION_POLICY, mitigations) ||
          !attributes.set(PROC_THREAD_ATTRIBUTE_CHILD_PROCESS_POLICY,
                          child_process_policy) ||
          !attributes.set_span(PROC_THREAD_ATTRIBUTE_JOB_LIST, std::span{jobs})) {
        return {.backend = nullptr,
                .error = IsolatedWorkerError::confinement_unavailable};
      }
#ifdef OHL_WINDOWS_ISOLATED_WORKER_TESTING
      record_launch_stage(
          windows::IsolatedWorkerLaunchStage::process_policy_created);
#endif

      STARTUPINFOEXW startup{};
      startup.StartupInfo.cb = static_cast<DWORD>(sizeof(startup));
      startup.StartupInfo.dwFlags = STARTF_USESHOWWINDOW;
      startup.StartupInfo.wShowWindow = SW_HIDE;
      startup.lpAttributeList = attributes.get();
      PROCESS_INFORMATION process_info{};
      const DWORD creation_flags = CREATE_SUSPENDED | CREATE_NO_WINDOW |
                                   CREATE_UNICODE_ENVIRONMENT |
                                   EXTENDED_STARTUPINFO_PRESENT;
      BOOL process_created = FALSE;
      {
#ifdef OHL_WINDOWS_ISOLATED_WORKER_TESTING
        record_launch_stage(
            windows::IsolatedWorkerLaunchStage::process_creation_pending);
#endif
        ScopedInheritableHandleWindow inherit_window{pipe.child};
        if (!inherit_window.valid()) {
          return {.backend = nullptr,
                  .error = IsolatedWorkerError::channel_creation_failed};
        }
        if (!trusted_path_still_matches(executable)) {
          inherit_window.close_parent_copy();
          return {.backend = nullptr,
                  .error = IsolatedWorkerError::service_identity_mismatch};
        }
        process_created = CreateProcessW(
            executable.path.c_str(), command_line.data(), nullptr, nullptr, TRUE,
            creation_flags, environment.data(), executable.directory.c_str(),
            &startup.StartupInfo, &process_info);
#ifdef OHL_WINDOWS_ISOLATED_WORKER_TESTING
        if (!process_created) {
          const DWORD create_error = GetLastError();
          g_last_create_process_error.store(create_error);
          g_last_create_process_failure.store(
              classify_create_process_failure(create_error));
        }
#endif
        // Whether creation succeeds or fails, no inheritable parent copy may
        // survive the serialized window.
        inherit_window.close_parent_copy();
      }
      if (!process_created) {
        return {.backend = nullptr,
                .error = IsolatedWorkerError::process_creation_failed};
      }
#ifdef OHL_WINDOWS_ISOLATED_WORKER_TESTING
      record_launch_stage(windows::IsolatedWorkerLaunchStage::process_created);
#endif
      UniqueHandle process{process_info.hProcess};
      UniqueHandle thread{process_info.hThread};
      ProcessCleanupGuard process_cleanup{process.get(), job.get()};

#ifdef OHL_WINDOWS_ISOLATED_WORKER_TESTING
      record_launch_stage(
          windows::IsolatedWorkerLaunchStage::image_verification_pending);
#endif
      if (!process_image_matches(process.get(), executable.identity)) {
        static_cast<void>(TerminateJobObject(job.get(),
                                             windows::kTerminationExitCode));
        return {.backend = nullptr,
                .error = IsolatedWorkerError::service_identity_mismatch};
      }
#ifdef OHL_WINDOWS_ISOLATED_WORKER_TESTING
      record_launch_stage(windows::IsolatedWorkerLaunchStage::image_verified);
      record_launch_stage(
          windows::IsolatedWorkerLaunchStage::machine_verification_pending);
      if (!process_machine_is_native_x64(process.get())) {
        static_cast<void>(TerminateJobObject(job.get(),
                                             windows::kTerminationExitCode));
        return {.backend = nullptr,
                .error = IsolatedWorkerError::confinement_unavailable};
      }
      record_launch_stage(windows::IsolatedWorkerLaunchStage::machine_verified);
      record_launch_stage(
          windows::IsolatedWorkerLaunchStage::token_verification_pending);
      if (!verify_lpac_token(process.get(), app_container_sid.get())) {
        static_cast<void>(TerminateJobObject(job.get(),
                                             windows::kTerminationExitCode));
        return {.backend = nullptr,
                .error = IsolatedWorkerError::confinement_unavailable};
      }
      record_launch_stage(windows::IsolatedWorkerLaunchStage::token_verified);
      record_launch_stage(
          windows::IsolatedWorkerLaunchStage::mitigation_verification_pending);
      if (!verify_mitigations(process.get())) {
        static_cast<void>(TerminateJobObject(job.get(),
                                             windows::kTerminationExitCode));
        return {.backend = nullptr,
                .error = IsolatedWorkerError::confinement_unavailable};
      }
      record_launch_stage(
          windows::IsolatedWorkerLaunchStage::mitigations_verified);
      record_launch_stage(
          windows::IsolatedWorkerLaunchStage::job_verification_pending);
      if (!verify_job(job.get(), process.get())) {
        static_cast<void>(TerminateJobObject(job.get(),
                                             windows::kTerminationExitCode));
        return {.backend = nullptr,
                .error = IsolatedWorkerError::confinement_unavailable};
      }
      record_launch_stage(windows::IsolatedWorkerLaunchStage::job_verified);
#else
      if (!process_machine_is_native_x64(process.get()) ||
          !verify_lpac_token(process.get(), app_container_sid.get()) ||
          !verify_mitigations(process.get()) ||
          !verify_job(job.get(), process.get())) {
        static_cast<void>(TerminateJobObject(job.get(),
                                             windows::kTerminationExitCode));
        return {.backend = nullptr,
                .error = IsolatedWorkerError::confinement_unavailable};
      }
#endif
      if (startup_deadline <= Clock::now()) {
        static_cast<void>(TerminateJobObject(job.get(),
                                             windows::kTerminationExitCode));
        return {.backend = nullptr, .error = IsolatedWorkerError::timeout};
      }
#ifdef OHL_WINDOWS_ISOLATED_WORKER_TESTING
      record_launch_stage(windows::IsolatedWorkerLaunchStage::resume_pending);
#endif
      if (ResumeThread(thread.get()) == std::numeric_limits<DWORD>::max()) {
        static_cast<void>(TerminateJobObject(job.get(),
                                             windows::kTerminationExitCode));
        return {.backend = nullptr,
                .error = IsolatedWorkerError::bootstrap_failed};
      }
#ifdef OHL_WINDOWS_ISOLATED_WORKER_TESTING
      record_launch_stage(windows::IsolatedWorkerLaunchStage::resumed);
#endif
      thread.reset();

      auto backend = std::make_unique<WindowsIsolatedWorkerBackend>();
      backend->adopt(std::move(process), std::move(job), std::move(pipe.server),
                     profile_guard.release());
      process_cleanup.dismiss();
      std::array<std::byte, kBootstrapReady.size()> bootstrap{};
#ifdef OHL_WINDOWS_ISOLATED_WORKER_TESTING
      record_launch_stage(windows::IsolatedWorkerLaunchStage::ready_pending);
#endif
      const IsolatedWorkerIoResult bootstrap_result =
          backend->read_exact(bootstrap, startup_deadline, {});
      if (bootstrap_result.error != IsolatedWorkerError::none ||
          bootstrap != kBootstrapReady) {
        IsolatedWorkerError error =
            bootstrap_result.error == IsolatedWorkerError::timeout
                ? IsolatedWorkerError::timeout
                : IsolatedWorkerError::bootstrap_failed;
        const IsolatedWorkerWaitResult cleanup =
            backend->terminate_and_wait(Clock::time_point::max());
        if (cleanup.error == IsolatedWorkerError::confinement_unavailable) {
          error = IsolatedWorkerError::confinement_unavailable;
        }
        backend.reset();
        return {.backend = nullptr, .error = error};
      }
#ifdef OHL_WINDOWS_ISOLATED_WORKER_TESTING
      record_launch_stage(windows::IsolatedWorkerLaunchStage::ready);
#endif

      return IsolatedWorkerBackendLaunchResult{
          .backend = std::move(backend),
          .error = IsolatedWorkerError::none};
    }();
    if (result.backend == nullptr && !profile_guard.cleanup_or_defer()) {
      result.error = IsolatedWorkerError::confinement_unavailable;
    }
    return result;
  } catch (const std::bad_alloc&) {
    return {.backend = nullptr,
            .error = IsolatedWorkerError::resource_exhausted};
  } catch (...) {
    return {.backend = nullptr,
            .error = IsolatedWorkerError::resource_exhausted};
  }
}

}  // namespace ohl::platform::detail
