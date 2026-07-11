#include "isolated_worker_windows_internal.hpp"

#ifndef NOMINMAX
#define NOMINMAX
#endif
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>

#include <array>
#include <cstddef>
#include <cstdint>
#include <limits>
#include <string_view>

#ifndef OHL_WINDOWS_TEST_WORKER_MODE
#define OHL_WINDOWS_TEST_WORKER_MODE 0
#endif

namespace {

using ohl::platform::detail::windows::kBootstrapReady;

[[nodiscard]] bool parse_handle(const std::wstring_view argument,
                                HANDLE& handle) noexcept {
  constexpr auto prefix =
      ohl::platform::detail::windows::kIpcHandleArgumentPrefix;
  if (!argument.starts_with(prefix) || argument.size() == prefix.size()) {
    return false;
  }
  std::uintptr_t value = 0;
  for (const wchar_t character : argument.substr(prefix.size())) {
    if (character < L'0' || character > L'9') {
      return false;
    }
    const auto digit = static_cast<std::uintptr_t>(character - L'0');
    if (value >
        (std::numeric_limits<std::uintptr_t>::max() - digit) / 10U) {
      return false;
    }
    value = value * 10U + digit;
  }
  if (value == 0) {
    return false;
  }
  handle = reinterpret_cast<HANDLE>(value);
  return handle != INVALID_HANDLE_VALUE;
}

// TokenIsLessPrivilegedAppContainer is not reliably queryable from user
// mode; the kernel marks LPAC tokens with the WIN://NOALLAPPPKG security
// attribute instead, so attest LPAC through that attribute.
[[nodiscard]] bool token_attributes_are_lpac(const HANDLE token) noexcept {
  alignas(CLAIM_SECURITY_ATTRIBUTES_INFORMATION)
      std::byte storage[16U * 1024U];
  DWORD returned = 0;
  if (GetTokenInformation(token, TokenSecurityAttributes, storage,
                          sizeof(storage), &returned) == FALSE) {
    return false;
  }
  const auto* information =
      reinterpret_cast<const CLAIM_SECURITY_ATTRIBUTES_INFORMATION*>(storage);
  if (information->Version !=
      CLAIM_SECURITY_ATTRIBUTES_INFORMATION_VERSION_V1) {
    return false;
  }
  for (DWORD index = 0; index < information->AttributeCount; ++index) {
    const CLAIM_SECURITY_ATTRIBUTE_V1& attribute =
        information->Attribute.pAttributeV1[index];
    if (attribute.Name == nullptr ||
        std::wstring_view{attribute.Name} != L"WIN://NOALLAPPPKG") {
      continue;
    }
    if (attribute.ValueCount == 0) {
      return false;
    }
    if (attribute.ValueType == CLAIM_SECURITY_ATTRIBUTE_TYPE_UINT64) {
      return attribute.Values.pUint64 != nullptr &&
             attribute.Values.pUint64[0] != 0U;
    }
    if (attribute.ValueType == CLAIM_SECURITY_ATTRIBUTE_TYPE_INT64) {
      return attribute.Values.pInt64 != nullptr &&
             attribute.Values.pInt64[0] != 0;
    }
    return false;
  }
  return false;
}

[[nodiscard]] bool token_contract_is_lpac() noexcept {
  HANDLE token = nullptr;
  if (!OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &token)) {
    return false;
  }
  DWORD app_container = 0;
  DWORD returned = 0;
  const bool app_container_ok =
      GetTokenInformation(token, TokenIsAppContainer, &app_container,
                          sizeof(app_container), &returned) != FALSE &&
      app_container != 0;
  const bool lpac_ok = token_attributes_are_lpac(token);
  CloseHandle(token);
  return app_container_ok && lpac_ok;
}

[[nodiscard]] bool overlapped_transfer(const HANDLE pipe, std::byte* data,
                                       const DWORD size,
                                       const bool write) noexcept {
  HANDLE event = CreateEventW(nullptr, TRUE, FALSE, nullptr);
  if (event == nullptr) {
    return false;
  }
  OVERLAPPED operation{};
  operation.hEvent = event;
  DWORD transferred = 0;
  const BOOL completed =
      write ? WriteFile(pipe, data, size, &transferred, &operation)
            : ReadFile(pipe, data, size, &transferred, &operation);
  bool success = completed != FALSE;
  if (!success && GetLastError() == ERROR_IO_PENDING) {
    success = GetOverlappedResult(pipe, &operation, &transferred, TRUE) != FALSE;
  }
  CloseHandle(event);
  return success && transferred > 0 && transferred <= size;
}

[[nodiscard]] bool write_bootstrap(const HANDLE pipe,
                                   const bool valid) noexcept {
  auto attestation = kBootstrapReady;
  if (!valid) {
    attestation.front() = std::byte{'X'};
  }
  return overlapped_transfer(pipe, attestation.data(),
                             static_cast<DWORD>(attestation.size()), true);
}

}  // namespace

int wmain(const int argument_count, wchar_t** const arguments) {
  namespace native = ohl::platform::detail::windows;
  if (argument_count != 4 || arguments == nullptr ||
      std::wstring_view{arguments[1]} != native::kMediaParserModeArgument ||
      std::wstring_view{arguments[3]} != native::kBootstrapArgument ||
      !token_contract_is_lpac()) {
    return 80;
  }

  HANDLE pipe = nullptr;
  if (!parse_handle(arguments[2], pipe)) {
    return 81;
  }

  if constexpr (OHL_WINDOWS_TEST_WORKER_MODE == 1) {
    return write_bootstrap(pipe, false) ? 0 : 82;
  }
  if constexpr (OHL_WINDOWS_TEST_WORKER_MODE == 2) {
    Sleep(INFINITE);
    return 83;
  }
  if (!write_bootstrap(pipe, true)) {
    return 84;
  }
  if constexpr (OHL_WINDOWS_TEST_WORKER_MODE == 3) {
    return 7;
  }

  std::array<std::byte, 4096> buffer{};
  while (true) {
    OVERLAPPED read{};
    HANDLE read_event = CreateEventW(nullptr, TRUE, FALSE, nullptr);
    if (read_event == nullptr) {
      return 85;
    }
    read.hEvent = read_event;
    DWORD transferred = 0;
    const BOOL completed = ReadFile(pipe, buffer.data(),
                                    static_cast<DWORD>(buffer.size()),
                                    &transferred, &read);
    BOOL read_ok = completed;
    if (!completed && GetLastError() == ERROR_IO_PENDING) {
      read_ok = GetOverlappedResult(pipe, &read, &transferred, TRUE);
    }
    const DWORD read_error = read_ok ? ERROR_SUCCESS : GetLastError();
    CloseHandle(read_event);
    if (!read_ok) {
      return read_error == ERROR_BROKEN_PIPE ||
                     read_error == ERROR_PIPE_NOT_CONNECTED
                 ? 0
                 : 86;
    }
    if (transferred == 0) {
      return 0;
    }
    if (!overlapped_transfer(pipe, buffer.data(), transferred, true)) {
      return 87;
    }
  }
}
