#include "ohl/media/import_cache.hpp"

#include <chrono>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <sstream>
#include <string>
#include <system_error>

namespace {

class TemporaryDirectory final {
 public:
  TemporaryDirectory() {
    const auto nonce = std::chrono::steady_clock::now().time_since_epoch().count();
    const auto created_path =
        std::filesystem::temp_directory_path() /
        ("open-half-life-import-cache-" + std::to_string(nonce));
    std::error_code error_code;
    if (!std::filesystem::create_directory(created_path, error_code) ||
        error_code) {
      return;
    }
    path_ = std::filesystem::canonical(created_path, error_code);
    if (error_code) {
      std::error_code cleanup_error;
      std::filesystem::remove_all(created_path, cleanup_error);
      path_.clear();
      return;
    }
    valid_ = true;
  }

  ~TemporaryDirectory() {
    std::error_code error_code;
    std::filesystem::remove_all(path_, error_code);
  }

  [[nodiscard]] bool valid() const noexcept { return valid_; }
  [[nodiscard]] const std::filesystem::path& path() const noexcept {
    return path_;
  }

 private:
  std::filesystem::path path_;
  bool valid_{false};
};

[[nodiscard]] bool write_text(const std::filesystem::path& path,
                              const std::string& text) {
  std::ofstream output{path, std::ios::binary};
  output << text;
  return output.good();
}

}  // namespace

int main() {
  TemporaryDirectory temporary;
  if (!temporary.valid()) {
    std::cerr << "failed to create import-cache test directory\n";
    return 1;
  }
  const auto source = temporary.path() / "private-source-name.iso";
  if (!write_text(source, "abc")) {
    std::cerr << "failed to create synthetic import source\n";
    return 1;
  }
  const ohl::media::IsoInspection inspection{
      .error = ohl::media::MediaError::none,
      .size_bytes = 3,
      .last_write_time = std::filesystem::last_write_time(source),
      .source_sha256 = "ba7816bf8f01cfea414140de5dae2223"
                       "b00361a396177a9cb410ff61f20015ad",
      .filesystem = "synthetic",
      .volume_label = "TEST\"LABEL",
  };
  const auto cache_root = temporary.path() / "cache";
  const auto first =
      ohl::media::prepare_import_cache(source, inspection, cache_root);
  constexpr std::string_view kExpectedDigest =
      "ba7816bf8f01cfea414140de5dae2223"
      "b00361a396177a9cb410ff61f20015ad";
  if (!first.valid() || first.cache_hit ||
      first.source_sha256 != kExpectedDigest ||
      first.source_directory != cache_root / "sources" / kExpectedDigest) {
    std::cerr << "new cache entry was not prepared: "
              << ohl::media::to_string(first.error) << '\n';
    return 1;
  }

  std::ifstream manifest_input{first.source_directory / "provenance.json"};
  std::ostringstream manifest_stream;
  manifest_stream << manifest_input.rdbuf();
  const auto manifest = manifest_stream.str();
  if (!manifest_input || manifest.find(kExpectedDigest) == std::string::npos ||
      manifest.find("TEST\\\"LABEL") == std::string::npos ||
      manifest.find("private-source-name") != std::string::npos ||
      manifest.find("not-imported") == std::string::npos) {
    std::cerr << "provenance manifest was missing or exposed the source path\n";
    return 1;
  }

  const auto second =
      ohl::media::prepare_import_cache(source, inspection, cache_root);
  if (!second.valid() || !second.cache_hit ||
      second.source_sha256 != first.source_sha256) {
    std::cerr << "matching provenance cache was not reused\n";
    return 1;
  }

  if (!write_text(first.source_directory / "provenance.json", "tampered\n")) {
    std::cerr << "failed to replace test provenance manifest\n";
    return 1;
  }
  const auto conflict =
      ohl::media::prepare_import_cache(source, inspection, cache_root);
  if (conflict.error != ohl::media::ImportCacheError::manifest_conflict) {
    std::cerr << "conflicting provenance manifest was not rejected\n";
    return 1;
  }

  auto changed_inspection = inspection;
  changed_inspection.size_bytes = 4;
  const auto changed =
      ohl::media::prepare_import_cache(source, changed_inspection, cache_root);
  if (changed.error != ohl::media::ImportCacheError::source_changed) {
    std::cerr << "changed source size was not rejected\n";
    return 1;
  }
  auto stale_inspection = inspection;
  stale_inspection.last_write_time -= std::chrono::seconds{1};
  const auto stale =
      ohl::media::prepare_import_cache(source, stale_inspection, cache_root);
  if (stale.error != ohl::media::ImportCacheError::source_changed) {
    std::cerr << "changed source timestamp was not rejected\n";
    return 1;
  }
  auto replaced_inspection = inspection;
  replaced_inspection.source_sha256 = std::string(64, '0');
  const auto replaced = ohl::media::prepare_import_cache(
      source, replaced_inspection, cache_root);
  if (replaced.error != ohl::media::ImportCacheError::source_changed) {
    std::cerr << "same-size source replacement was not rejected\n";
    return 1;
  }
  const auto missing = ohl::media::prepare_import_cache(
      temporary.path() / "missing.iso", inspection, cache_root);
  if (missing.error != ohl::media::ImportCacheError::source_changed) {
    std::cerr << "missing import source returned the wrong error\n";
    return 1;
  }
  const auto relative = ohl::media::prepare_import_cache(
      source, inspection, std::filesystem::path{"relative-cache"});
  if (relative.error != ohl::media::ImportCacheError::invalid_request) {
    std::cerr << "relative cache root was not rejected\n";
    return 1;
  }

  const auto link = temporary.path() / "linked-cache";
  std::error_code error_code;
  std::filesystem::create_directory_symlink(cache_root, link, error_code);
#if !defined(_WIN32)
  if (error_code) {
    std::cerr << "failed to create linked-cache rejection fixture\n";
    return 1;
  }
#endif
  if (!error_code) {
    const auto linked =
        ohl::media::prepare_import_cache(source, inspection, link);
    if (linked.error != ohl::media::ImportCacheError::unsafe_cache_path) {
      std::cerr << "linked cache root was not rejected\n";
      return 1;
    }
  }

  const auto replacement = temporary.path() / "replacement.iso";
  if (!write_text(replacement, "xyz")) {
    std::cerr << "failed to create same-size replacement source\n";
    return 1;
  }
  std::filesystem::last_write_time(replacement, inspection.last_write_time,
                                   error_code);
  if (error_code || !std::filesystem::remove(source, error_code) || error_code) {
    std::cerr << "failed to prepare same-time replacement source\n";
    return 1;
  }
  std::filesystem::rename(replacement, source, error_code);
  if (error_code) {
    std::cerr << "failed to install same-time replacement source\n";
    return 1;
  }
  const auto physically_replaced =
      ohl::media::prepare_import_cache(source, inspection, cache_root);
  if (physically_replaced.error !=
      ohl::media::ImportCacheError::source_changed) {
    std::cerr << "same-size, same-time source replacement was not rejected\n";
    return 1;
  }
  return 0;
}
