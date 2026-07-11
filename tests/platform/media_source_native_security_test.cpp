#include <cstddef>
#include <cstdint>
#include <filesystem>
#include <memory>
#include <span>

// This target compiles one native backend directly so it can verify the
// implementation-private descriptor/handle inheritance state without adding a
// native-handle accessor to the public MediaSource API.
#define private public
#include "ohl/platform/media_source.hpp"
#undef private

#if defined(_WIN32)
#include "../../src/platform/src/media_source_windows.cpp"
#elif defined(__linux__) || defined(__APPLE__)
#include "../../src/platform/src/media_source_posix.cpp"
#endif

#include <array>
#include <charconv>
#include <chrono>
#include <fstream>
#include <iostream>
#include <string>
#include <string_view>
#include <system_error>

#if defined(_WIN32)
#include <windows.h>
#else
#include <cerrno>
#include <fcntl.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>
#endif

namespace {

class TemporaryDirectory final {
 public:
  TemporaryDirectory() {
    std::error_code error;
    const auto parent = std::filesystem::temp_directory_path(error);
    if (error) {
      return;
    }
    const auto nonce = std::chrono::steady_clock::now()
                           .time_since_epoch()
                           .count();
    for (unsigned int attempt = 0; attempt < 128; ++attempt) {
      auto candidate = parent / ("ohl-media-source-native-test-" +
                                 std::to_string(nonce) + "-" +
                                 std::to_string(attempt));
      error.clear();
      if (std::filesystem::create_directory(candidate, error)) {
        path_ = std::move(candidate);
        return;
      }
      if (error && error != std::errc::file_exists) {
        return;
      }
    }
  }

  ~TemporaryDirectory() {
    if (!path_.empty()) {
      std::error_code error;
      std::filesystem::remove_all(path_, error);
    }
  }

  TemporaryDirectory(const TemporaryDirectory&) = delete;
  TemporaryDirectory& operator=(const TemporaryDirectory&) = delete;

  [[nodiscard]] bool valid() const noexcept { return !path_.empty(); }
  [[nodiscard]] const std::filesystem::path& path() const noexcept {
    return path_;
  }

 private:
  std::filesystem::path path_;
};

[[nodiscard]] int fail(const std::string_view message) {
  std::cerr << "media source native security test failed: " << message << '\n';
  return 1;
}

template <typename Integer>
[[nodiscard]] bool parse_integer(const std::string_view text,
                                 Integer& value) noexcept {
  const auto result =
      std::from_chars(text.data(), text.data() + text.size(), value);
  return result.ec == std::errc{} && result.ptr == text.data() + text.size();
}

[[nodiscard]] bool create_fixture(const std::filesystem::path& path) {
  std::ofstream output{path, std::ios::binary | std::ios::trunc};
  output << "OHL synthetic native inheritance fixture\n";
  return output.good();
}

#if defined(_WIN32)

[[nodiscard]] int check_inherited_handle(const int argument_count,
                                         const char* const arguments[]) {
  if (argument_count != 6) {
    return 2;
  }
  std::uintptr_t handle_value = 0;
  unsigned long volume_serial = 0;
  unsigned long file_index_high = 0;
  unsigned long file_index_low = 0;
  if (!parse_integer(arguments[2], handle_value) ||
      !parse_integer(arguments[3], volume_serial) ||
      !parse_integer(arguments[4], file_index_high) ||
      !parse_integer(arguments[5], file_index_low)) {
    return 2;
  }

  BY_HANDLE_FILE_INFORMATION information{};
  const auto handle = reinterpret_cast<HANDLE>(handle_value);
  if (GetFileInformationByHandle(handle, &information) != 0 &&
      information.dwVolumeSerialNumber == volume_serial &&
      information.nFileIndexHigh == file_index_high &&
      information.nFileIndexLow == file_index_low) {
    return 1;
  }
  return 0;
}

[[nodiscard]] int test_native_inheritance() {
  TemporaryDirectory temporary;
  if (!temporary.valid()) {
    return fail("temporary directory creation");
  }
  const auto fixture = temporary.path() / "inheritance.fixture";
  if (!create_fixture(fixture)) {
    return fail("fixture creation");
  }
  const auto opened = ohl::platform::open_media_source(fixture);
  if (!opened.valid()) {
    return fail("source acquisition");
  }

  const auto handle = opened.source->implementation_->handle;
  DWORD handle_flags = 0;
  if (GetHandleInformation(handle, &handle_flags) == 0 ||
      (handle_flags & HANDLE_FLAG_INHERIT) != 0) {
    return fail("source handle is inheritable");
  }
  BY_HANDLE_FILE_INFORMATION information{};
  if (GetFileInformationByHandle(handle, &information) == 0) {
    return fail("source identity query");
  }

  std::array<wchar_t, 32'768> executable{};
  const auto executable_size = GetModuleFileNameW(
      nullptr, executable.data(), static_cast<DWORD>(executable.size()));
  if (executable_size == 0 || executable_size >= executable.size()) {
    return fail("child executable query");
  }
  std::wstring command_line =
      L"\"" + std::wstring{executable.data(), executable_size} +
      L"\" --check-inherited-handle " +
      std::to_wstring(reinterpret_cast<std::uintptr_t>(handle)) + L" " +
      std::to_wstring(information.dwVolumeSerialNumber) + L" " +
      std::to_wstring(information.nFileIndexHigh) + L" " +
      std::to_wstring(information.nFileIndexLow);

  STARTUPINFOW startup{};
  startup.cb = sizeof(startup);
  PROCESS_INFORMATION process{};
  if (CreateProcessW(nullptr, command_line.data(), nullptr, nullptr, TRUE, 0,
                     nullptr, nullptr, &startup, &process) == 0) {
    return fail("child process creation");
  }
  (void)CloseHandle(process.hThread);
  const auto wait_result = WaitForSingleObject(process.hProcess, 10'000);
  DWORD exit_code = 1;
  const bool child_passed =
      wait_result == WAIT_OBJECT_0 &&
      GetExitCodeProcess(process.hProcess, &exit_code) != 0 && exit_code == 0;
  if (wait_result != WAIT_OBJECT_0) {
    (void)TerminateProcess(process.hProcess, 1);
    (void)WaitForSingleObject(process.hProcess, 10'000);
  }
  (void)CloseHandle(process.hProcess);
  return child_passed ? 0 : fail("source handle crossed child execution");
}

#else

[[nodiscard]] int check_inherited_descriptor(const int argument_count,
                                             const char* const arguments[]) {
  if (argument_count != 5) {
    return 2;
  }
  int descriptor = -1;
  std::uintmax_t expected_device = 0;
  std::uintmax_t expected_inode = 0;
  if (!parse_integer(arguments[2], descriptor) || descriptor < 0 ||
      !parse_integer(arguments[3], expected_device) ||
      !parse_integer(arguments[4], expected_inode)) {
    return 2;
  }

  struct stat status {};
  if (::fstat(descriptor, &status) == 0 &&
      static_cast<std::uintmax_t>(status.st_dev) == expected_device &&
      static_cast<std::uintmax_t>(status.st_ino) == expected_inode) {
    return 1;
  }
  return 0;
}

[[nodiscard]] int test_native_inheritance(const char* const executable_name) {
  TemporaryDirectory temporary;
  if (!temporary.valid()) {
    return fail("temporary directory creation");
  }
  const auto fixture = temporary.path() / "inheritance.fixture";
  if (!create_fixture(fixture)) {
    return fail("fixture creation");
  }
  const auto opened = ohl::platform::open_media_source(fixture);
  if (!opened.valid()) {
    return fail("source acquisition");
  }

  const int descriptor = opened.source->implementation_->descriptor;
  const int descriptor_flags = ::fcntl(descriptor, F_GETFD);
  if (descriptor_flags < 0 || (descriptor_flags & FD_CLOEXEC) == 0) {
    return fail("source descriptor lacks close-on-exec");
  }
  struct stat status {};
  if (::fstat(descriptor, &status) != 0) {
    return fail("source identity query");
  }
  std::error_code error;
  const auto executable =
      std::filesystem::canonical(std::filesystem::path{executable_name}, error);
  if (error) {
    return fail("child executable resolution");
  }
  const auto descriptor_text = std::to_string(descriptor);
  const auto device_text =
      std::to_string(static_cast<std::uintmax_t>(status.st_dev));
  const auto inode_text =
      std::to_string(static_cast<std::uintmax_t>(status.st_ino));

  const auto child = ::fork();
  if (child < 0) {
    return fail("child process creation");
  }
  if (child == 0) {
    ::execl(executable.c_str(), executable.c_str(),
            "--check-inherited-descriptor", descriptor_text.c_str(),
            device_text.c_str(), inode_text.c_str(),
            static_cast<char*>(nullptr));
    ::_exit(125);
  }
  int child_status = 0;
  if (::waitpid(child, &child_status, 0) != child ||
      !WIFEXITED(child_status) || WEXITSTATUS(child_status) != 0) {
    return fail("source descriptor crossed child execution");
  }
  return 0;
}

#endif

}  // namespace

int main(const int argument_count, const char* const arguments[]) {
#if defined(_WIN32)
  if (argument_count > 1 &&
      std::string_view{arguments[1]} == "--check-inherited-handle") {
    return check_inherited_handle(argument_count, arguments);
  }
  return test_native_inheritance();
#else
  if (argument_count > 1 &&
      std::string_view{arguments[1]} == "--check-inherited-descriptor") {
    return check_inherited_descriptor(argument_count, arguments);
  }
  return test_native_inheritance(arguments[0]);
#endif
}
