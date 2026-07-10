#include "ohl/core/log.hpp"
#include "ohl/media/iso_inspector.hpp"
#include "ohl/platform/platform.hpp"
#include "ohl/vfs/udf_archive.hpp"

#include <filesystem>
#include <iostream>
#include <optional>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace {

constexpr std::string_view kApplicationName{"Open Half-Life"};

[[nodiscard]] std::string path_to_utf8(const std::filesystem::path& path) {
  const auto encoded = path.u8string();
  return {reinterpret_cast<const char*>(encoded.data()), encoded.size()};
}

[[nodiscard]] std::filesystem::path path_from_utf8(
    const std::string_view encoded) {
  const std::u8string value{
      reinterpret_cast<const char8_t*>(encoded.data()), encoded.size()};
  return std::filesystem::path{value};
}

void print_usage() {
  std::cout << "Usage: open-half-life [--iso PATH] [--] [PATH]\n";
}

struct ParseResult {
  std::optional<std::filesystem::path> iso_path;
  int exit_code{-1};
};

[[nodiscard]] ParseResult parse_iso_path(
    const std::vector<std::filesystem::path>& arguments) {
  ParseResult result;
  bool options_enabled = true;
  for (std::size_t index = 1; index < arguments.size(); ++index) {
    const auto& argument_path = arguments[index];
    const auto argument = path_to_utf8(argument_path);
    if (options_enabled && (argument == "--help" || argument == "-h")) {
      print_usage();
      result.exit_code = 0;
      return result;
    }
    if (options_enabled && argument == "--version") {
      std::cout << kApplicationName << ' ' << OHL_VERSION << '\n';
      result.exit_code = 0;
      return result;
    }
    if (options_enabled && argument == "--") {
      options_enabled = false;
      continue;
    }

    std::filesystem::path iso_path;
    if (options_enabled && argument == "--iso") {
      ++index;
      if (index >= arguments.size()) {
        std::cerr << "--iso requires a path\n";
        result.exit_code = 2;
        return result;
      }
      iso_path = arguments[index];
    } else if (options_enabled && argument.starts_with("--iso=")) {
      iso_path = path_from_utf8(std::string_view{argument}.substr(6));
    } else if (options_enabled && argument.starts_with('-')) {
      std::cerr << "Unknown option: " << argument << '\n';
      print_usage();
      result.exit_code = 2;
      return result;
    } else {
      iso_path = argument_path;
    }

    if (iso_path.empty() || result.iso_path.has_value()) {
      std::cerr << (iso_path.empty() ? "ISO path must not be empty\n"
                                     : "Only one ISO path may be supplied\n");
      result.exit_code = 2;
      return result;
    }
    result.iso_path = std::move(iso_path);
  }
  return result;
}

[[nodiscard]] std::optional<std::filesystem::path> prompt_for_iso() {
  std::cout << "Path to a legally obtained Half-Life ISO: " << std::flush;
#if defined(_WIN32)
  std::wstring path_text;
  if (!std::getline(std::wcin, path_text) || path_text.empty()) {
    return std::nullopt;
  }
#else
  std::string path_text;
  if (!std::getline(std::cin, path_text) || path_text.empty()) {
    return std::nullopt;
  }
#endif
  return std::filesystem::path{path_text};
}

int run(const std::vector<std::filesystem::path>& arguments) {
  auto parse_result = parse_iso_path(arguments);
  if (parse_result.exit_code >= 0) {
    return parse_result.exit_code;
  }

  if (!parse_result.iso_path.has_value()) {
    parse_result.iso_path = prompt_for_iso();
    if (!parse_result.iso_path.has_value()) {
      std::cerr << "No ISO path provided. Use --iso PATH.\n";
      return 2;
    }
  }

  const auto platform = ohl::platform::current_platform();
  ohl::core::log(ohl::core::LogLevel::info,
                 std::string{kApplicationName} + " " + OHL_VERSION);
  ohl::core::log(
      ohl::core::LogLevel::info,
      std::string{"Platform: "} +
          std::string{ohl::platform::to_string(platform.operating_system)} +
          " " + std::string{ohl::platform::to_string(platform.architecture)});

  const auto inspection =
      ohl::media::inspect_iso(*parse_result.iso_path);
  if (!inspection.valid()) {
    ohl::core::log(ohl::core::LogLevel::error,
                   std::string{"Media preflight failed: "} +
                       std::string{ohl::media::to_string(inspection.error)});
    return 1;
  }

  ohl::vfs::UdfArchive archive;
  if (archive.open(*parse_result.iso_path) != ohl::vfs::VfsError::none) {
    ohl::core::log(ohl::core::LogLevel::error,
                   "Media is not a readable UDF filesystem.");
    return 1;
  }
  const auto root = archive.list("/");
  if (root.error != ohl::vfs::VfsError::none) {
    ohl::core::log(ohl::core::LogLevel::error,
                   "The UDF root directory could not be read.");
    return 1;
  }

  ohl::core::log(ohl::core::LogLevel::info,
                 std::string{"Mounted read-only UDF image ("} +
                     std::to_string(inspection.size_bytes) + " bytes)");
  const auto label = archive.volume_label();
  if (!label.empty()) {
    ohl::core::log(ohl::core::LogLevel::info,
                   std::string{"Volume label: "} + label);
  }
  ohl::core::log(ohl::core::LogLevel::info,
                 std::string{"Root entries: "} +
                     std::to_string(root.entries.size()));

  ohl::core::log(ohl::core::LogLevel::info,
                 "Media import is not implemented yet; nothing was executed.");
  return 0;
}

}  // namespace

#if defined(_WIN32)
int wmain(const int argument_count, const wchar_t* const arguments[]) {
#else
int main(const int argument_count, const char* const arguments[]) {
#endif
  std::vector<std::filesystem::path> paths;
  paths.reserve(static_cast<std::size_t>(argument_count));
  for (int index = 0; index < argument_count; ++index) {
    paths.emplace_back(arguments[index]);
  }
  return run(paths);
}
