#include "ohl/vfs/udf_archive.hpp"

#include <filesystem>
#include <array>
#include <cstddef>
#include <iostream>
#include <string>

namespace {

int expect_path(const std::string& input, const std::string& expected) {
  const auto actual = ohl::vfs::normalize_path(input);
  if (!actual.has_value() || *actual != expected) {
    std::cerr << "unexpected normalized path for '" << input << "'\n";
    return 1;
  }
  return 0;
}

int expect_rejected(const std::string& input) {
  if (ohl::vfs::normalize_path(input).has_value()) {
    std::cerr << "unsafe path was accepted: '" << input << "'\n";
    return 1;
  }
  return 0;
}

}  // namespace

int main(const int argument_count, const char* const arguments[]) {
  if (expect_path("", "/") != 0 || expect_path("/", "/") != 0 ||
      expect_path("Media/preview.mpg", "/Media/preview.mpg") != 0 ||
      expect_path("//Media\\preview.mpg", "/Media/preview.mpg") != 0 ||
      expect_rejected("../data1.cab") != 0 ||
      expect_rejected("Media/./preview.mpg") != 0 ||
      expect_rejected("Media/../data1.cab") != 0 ||
      expect_rejected(std::string{"safe\0hidden", 11}) != 0 ||
      !ohl::vfs::is_single_path_component("data1.cab") ||
      ohl::vfs::is_single_path_component("../data1.cab") ||
      ohl::vfs::is_single_path_component("Media/data1.cab") ||
      ohl::vfs::is_single_path_component("Media\\data1.cab") ||
      ohl::vfs::is_single_path_component(std::string{"safe\0hidden", 11})) {
    std::cerr << "single-component path validation failed\n";
    return 1;
  }

  ohl::vfs::UdfArchive archive;
  if (archive.is_open()) {
    std::cerr << "new archive unexpectedly reports open\n";
    return 1;
  }
  if (archive.list("/").error != ohl::vfs::VfsError::not_open) {
    std::cerr << "closed archive listing returned the wrong error\n";
    return 1;
  }
  if (archive.open(std::filesystem::path{"does-not-exist.iso"}) !=
      ohl::vfs::VfsError::open_failed) {
    std::cerr << "missing archive did not fail to open\n";
    return 1;
  }
  if (archive.open_file("anything") != nullptr) {
    std::cerr << "closed archive unexpectedly opened a file\n";
    return 1;
  }

  if (argument_count == 2) {
    if (archive.open(std::filesystem::path{arguments[1]}) !=
        ohl::vfs::VfsError::none) {
      std::cerr << "integration image did not mount\n";
      return 1;
    }
    const auto root = archive.list("/");
    if (root.error != ohl::vfs::VfsError::none || root.entries.empty()) {
      std::cerr << "integration image root could not be listed\n";
      return 1;
    }
    for (const auto& entry : root.entries) {
      if (entry.type == ohl::vfs::EntryType::file && entry.size_bytes > 0) {
        auto file = archive.open_file_at("/", entry.name);
        std::array<std::byte, 1> byte{};
        if (!file || file->size() != entry.size_bytes) {
          std::cerr << "integration image file could not be read\n";
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
    std::cerr << "integration image had no non-empty root file\n";
    return 1;
  }
  return 0;
}
