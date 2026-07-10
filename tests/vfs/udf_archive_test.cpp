#include "ohl/vfs/udf_archive.hpp"

#include <array>
#include <cstddef>
#include <filesystem>
#include <iostream>
#include <string>

namespace {

constexpr const char* kSyntheticDirectory = "ohl-synthetic-fixture";
constexpr const char* kSyntheticFile = "sentinel.fixture";

int expect_path(const std::string& input, const std::string& expected) {
  const auto actual = ohl::vfs::normalize_path(input);
  if (!actual.has_value() || *actual != expected) {
    std::cerr << "unexpected normalized synthetic path\n";
    return 1;
  }
  return 0;
}

int expect_rejected(const std::string& input) {
  if (ohl::vfs::normalize_path(input).has_value()) {
    std::cerr << "unsafe synthetic path was accepted\n";
    return 1;
  }
  return 0;
}

}  // namespace

int main(const int argument_count, const char* const arguments[]) {
  const std::string relative_path =
      std::string{kSyntheticDirectory} + '/' + kSyntheticFile;
  const std::string absolute_path = '/' + relative_path;
  const std::string mixed_separator_path =
      std::string{"//"} + kSyntheticDirectory + '\\' + kSyntheticFile;
  const std::string traversal_path = std::string{"../"} + kSyntheticFile;
  const std::string dotted_path =
      std::string{kSyntheticDirectory} + "/./" + kSyntheticFile;
  const std::string parent_path =
      std::string{kSyntheticDirectory} + "/../" + kSyntheticFile;

  if (expect_path("", "/") != 0 || expect_path("/", "/") != 0 ||
      expect_path(relative_path, absolute_path) != 0 ||
      expect_path(mixed_separator_path, absolute_path) != 0 ||
      expect_rejected(traversal_path) != 0 ||
      expect_rejected(dotted_path) != 0 ||
      expect_rejected(parent_path) != 0 ||
      expect_rejected(std::string{"safe\0hidden", 11}) != 0 ||
      !ohl::vfs::is_single_path_component(kSyntheticFile) ||
      ohl::vfs::is_single_path_component(traversal_path) ||
      ohl::vfs::is_single_path_component(relative_path) ||
      ohl::vfs::is_single_path_component(mixed_separator_path) ||
      ohl::vfs::is_single_path_component(std::string{"safe\0hidden", 11})) {
    std::cerr << "synthetic path validation failed\n";
    return 1;
  }

  ohl::vfs::UdfArchive archive;
  if (archive.is_open()) {
    std::cerr << "new archive unexpectedly reports open\n";
    return 1;
  }
  if (archive.share().is_open()) {
    std::cerr << "shared closed archive unexpectedly reports open\n";
    return 1;
  }
  if (archive.list("/").error != ohl::vfs::VfsError::not_open) {
    std::cerr << "closed archive listing returned the wrong error\n";
    return 1;
  }
  if (archive.open(std::filesystem::path{
          "ohl-synthetic-missing-image.fixture"}) !=
      ohl::vfs::VfsError::open_failed) {
    std::cerr << "missing synthetic archive did not fail to open\n";
    return 1;
  }
  if (archive.open_file(kSyntheticFile) != nullptr) {
    std::cerr << "closed archive unexpectedly opened a file\n";
    return 1;
  }

  if (argument_count == 2) {
    if (archive.open(std::filesystem::path{arguments[1]}) !=
        ohl::vfs::VfsError::none) {
      std::cerr << "runtime integration image did not mount\n";
      return 1;
    }
    const auto root = archive.list("/");
    if (root.error != ohl::vfs::VfsError::none || root.entries.empty()) {
      std::cerr << "runtime integration root could not be listed\n";
      return 1;
    }
    for (const auto& entry : root.entries) {
      if (entry.type == ohl::vfs::EntryType::file && entry.size_bytes > 0) {
        auto file = archive.open_file_at("/", entry.name);
        std::array<std::byte, 1> byte{};
        if (!file || file->size() != entry.size_bytes) {
          std::cerr << "runtime integration file could not be read\n";
          return 1;
        }
        archive.close();
        if (file->read(byte) != 1) {
          std::cerr << "open file did not retain its archive lifetime\n";
          return 1;
        }
        return 0;
      }
    }
    std::cerr << "runtime integration image had no non-empty root file\n";
    return 1;
  }
  return 0;
}
