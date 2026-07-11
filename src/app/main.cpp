#include "ohl/core/log.hpp"
#include "ohl/media/import_cache.hpp"
#include "ohl/media/iso_inspector.hpp"
#include "ohl/platform/media_source.hpp"
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
  std::cout << "Usage: open-half-life [--iso PATH] [--cache PATH] [--] [PATH]\n";
}

struct ParseResult {
  std::optional<std::filesystem::path> iso_path;
  std::optional<std::filesystem::path> cache_path;
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

    std::filesystem::path value_path;
    enum class PathKind { iso, cache };
    auto path_kind = PathKind::iso;
    if (options_enabled && argument == "--iso") {
      ++index;
      if (index >= arguments.size()) {
        std::cerr << "--iso requires a path\n";
        result.exit_code = 2;
        return result;
      }
      value_path = arguments[index];
    } else if (options_enabled && argument.starts_with("--iso=")) {
      value_path = path_from_utf8(std::string_view{argument}.substr(6));
    } else if (options_enabled && argument == "--cache") {
      ++index;
      if (index >= arguments.size()) {
        std::cerr << "--cache requires a path\n";
        result.exit_code = 2;
        return result;
      }
      value_path = arguments[index];
      path_kind = PathKind::cache;
    } else if (options_enabled && argument.starts_with("--cache=")) {
      value_path = path_from_utf8(std::string_view{argument}.substr(8));
      path_kind = PathKind::cache;
    } else if (options_enabled && argument.starts_with('-')) {
      std::cerr << "Unknown option: " << argument << '\n';
      print_usage();
      result.exit_code = 2;
      return result;
    } else {
      value_path = argument_path;
    }

    auto& destination = path_kind == PathKind::iso ? result.iso_path
                                                    : result.cache_path;
    if (value_path.empty() || destination.has_value()) {
      std::cerr << (value_path.empty()
                        ? (path_kind == PathKind::iso
                               ? "ISO path must not be empty\n"
                               : "Cache path must not be empty\n")
                        : (path_kind == PathKind::iso
                               ? "Only one ISO path may be supplied\n"
                               : "Only one cache path may be supplied\n"));
      result.exit_code = 2;
      return result;
    }
    destination = std::move(value_path);
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

  auto opened_source =
      ohl::platform::open_media_source(*parse_result.iso_path);
  parse_result.iso_path.reset();
  if (!opened_source.valid()) {
    const std::string_view error =
        opened_source.error == ohl::platform::MediaSourceError::not_found
            ? "file not found"
            : "media could not be opened";
    ohl::core::log(ohl::core::LogLevel::error,
                   std::string{"Media preflight failed: "} +
                       std::string{error});
    return 1;
  }

  auto validation =
      ohl::media::validate_iso(std::move(opened_source.source));
  if (!validation.valid()) {
    ohl::core::log(ohl::core::LogLevel::error,
                   std::string{"Media preflight failed: "} +
                       std::string{ohl::media::to_string(validation.error)});
    return 1;
  }
  auto validated = std::move(*validation.media);

  ohl::vfs::UdfArchive archive;
  if (archive.open(validated.source()) != ohl::vfs::VfsError::none) {
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
                 "Mounted read-only UDF image.");

  if (!parse_result.cache_path.has_value()) {
    parse_result.cache_path = ohl::platform::default_cache_directory();
  }
  if (!parse_result.cache_path.has_value()) {
    ohl::core::log(
        ohl::core::LogLevel::error,
        "No user cache directory is available. Supply --cache PATH.");
    return 1;
  }
  const auto cache = ohl::media::prepare_import_cache(
      validated, *parse_result.cache_path);
  if (!cache.valid()) {
    ohl::core::log(ohl::core::LogLevel::error,
                   std::string{"Media cache preparation failed: "} +
                       std::string{ohl::media::to_string(cache.error)});
    return 1;
  }
  ohl::core::log(
      ohl::core::LogLevel::info,
      std::string{cache.cache_hit ? "Reused" : "Prepared"} +
          " metadata-only media cache.");
  ohl::core::log(ohl::core::LogLevel::info,
                 "Payload import is not implemented yet; no media executable "
                 "was run.");
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
