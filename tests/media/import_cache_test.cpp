#include "ohl/media/import_cache.hpp"

#include "import_cache_internal.hpp"
#include "source_stability_internal.hpp"

#include <array>
#include <chrono>
#include <cstddef>
#include <cstdint>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <span>
#include <sstream>
#include <string>
#include <system_error>
#include <utility>
#include <vector>

namespace {

constexpr std::size_t kSectorSize = 2'048;
constexpr std::size_t kSectorCount = 300;
using Sector = std::span<std::byte, kSectorSize>;

class TemporaryDirectory final {
 public:
  TemporaryDirectory() {
    const auto nonce =
        std::chrono::steady_clock::now().time_since_epoch().count();
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

void write_little_u16(const Sector sector, const std::size_t offset,
                      const std::uint16_t value) {
  sector[offset] = static_cast<std::byte>(value & 0xffU);
  sector[offset + 1] = static_cast<std::byte>((value >> 8U) & 0xffU);
}

void write_little_u32(const Sector sector, const std::size_t offset,
                      const std::uint32_t value) {
  sector[offset] = static_cast<std::byte>(value & 0xffU);
  sector[offset + 1] = static_cast<std::byte>((value >> 8U) & 0xffU);
  sector[offset + 2] = static_cast<std::byte>((value >> 16U) & 0xffU);
  sector[offset + 3] = static_cast<std::byte>((value >> 24U) & 0xffU);
}

[[nodiscard]] std::uint16_t crc_itu_t(
    const std::span<const std::byte> bytes) {
  std::uint16_t crc = 0;
  for (const auto value : bytes) {
    crc ^= static_cast<std::uint16_t>(
        static_cast<std::uint16_t>(std::to_integer<std::uint8_t>(value))
        << 8U);
    for (int bit = 0; bit < 8; ++bit) {
      const bool high_bit_set = (crc & 0x8000U) != 0;
      crc = static_cast<std::uint16_t>(crc << 1U);
      if (high_bit_set) {
        crc ^= 0x1021U;
      }
    }
  }
  return crc;
}

void finish_tag(const Sector sector, const std::uint16_t identifier,
                const std::uint32_t location,
                const std::uint16_t crc_length = 496) {
  write_little_u16(sector, 0, identifier);
  write_little_u16(sector, 2, 2);
  write_little_u16(sector, 6, 1);
  write_little_u16(sector, 10, crc_length);
  write_little_u32(sector, 12, location);
  write_little_u16(
      sector, 8,
      crc_itu_t(std::span<const std::byte>{sector}.subspan(16, crc_length)));

  std::uint8_t checksum = 0;
  for (std::size_t index = 0; index < 16; ++index) {
    if (index != 4) {
      checksum = static_cast<std::uint8_t>(
          checksum + std::to_integer<std::uint8_t>(sector[index]));
    }
  }
  sector[4] = static_cast<std::byte>(checksum);
}

void set_identifier(const Sector sector, const std::string& identifier) {
  sector[0] = std::byte{0};
  for (std::size_t index = 0; index < identifier.size(); ++index) {
    sector[index + 1] = static_cast<std::byte>(identifier[index]);
  }
  sector[6] = std::byte{1};
}

[[nodiscard]] Sector sector_at(std::vector<std::byte>& image,
                               const std::size_t index) {
  return Sector{image.data() + index * kSectorSize, kSectorSize};
}

[[nodiscard]] std::vector<std::byte> make_valid_image() {
  std::vector<std::byte> image(kSectorCount * kSectorSize);
  set_identifier(sector_at(image, 18), "BEA01");
  set_identifier(sector_at(image, 19), "NSR02");
  set_identifier(sector_at(image, 20), "TEA01");

  auto primary = sector_at(image, 32);
  const std::string volume_label{"CACHE\"TEST"};
  primary[24] = std::byte{8};
  for (std::size_t index = 0; index < volume_label.size(); ++index) {
    primary[25 + index] = static_cast<std::byte>(volume_label[index]);
  }
  primary[55] = static_cast<std::byte>(volume_label.size() + 1);
  finish_tag(primary, 1, 32);
  finish_tag(sector_at(image, 33), 5, 33);
  auto logical = sector_at(image, 34);
  write_little_u32(logical, 212, 2'048);
  finish_tag(logical, 6, 34, 424);
  finish_tag(sector_at(image, 35), 8, 35);

  auto anchor = sector_at(image, 256);
  write_little_u32(anchor, 16, 16 * 2'048);
  write_little_u32(anchor, 20, 32);
  write_little_u32(anchor, 24, 16 * 2'048);
  write_little_u32(anchor, 28, 48);
  finish_tag(anchor, 2, 256);
  return image;
}

[[nodiscard]] bool write_image(const std::filesystem::path& path,
                               const std::vector<std::byte>& image) {
  std::ofstream output{path, std::ios::binary};
  output.write(reinterpret_cast<const char*>(image.data()),
               static_cast<std::streamsize>(image.size()));
  return output.good();
}

[[nodiscard]] bool write_text(const std::filesystem::path& path,
                              const std::string& text) {
  std::ofstream output{path, std::ios::binary};
  output << text;
  return output.good();
}

[[nodiscard]] bool read_text(const std::filesystem::path& path,
                             std::string& text) {
  std::ifstream input{path, std::ios::binary};
  std::ostringstream stream;
  stream << input.rdbuf();
  text = stream.str();
  return input.good() || input.eof();
}

struct PublishRaceContext {
  std::filesystem::path destination;
  std::string conflicting_contents;
  bool invoked{false};
  bool conflict_created{false};
};

void create_publish_conflict(void* const context) noexcept {
  auto& race = *static_cast<PublishRaceContext*>(context);
  race.invoked = true;
  try {
    race.conflict_created =
        write_text(race.destination, race.conflicting_contents);
  } catch (...) {
    race.conflict_created = false;
  }
}

}  // namespace

int main() {
  TemporaryDirectory temporary;
  if (!temporary.valid()) {
    std::cerr << "failed to create import-cache test directory\n";
    return 1;
  }

  const auto source = temporary.path() / "private-source-name.iso";
  if (!write_image(source, make_valid_image())) {
    std::cerr << "failed to create synthetic import source\n";
    return 1;
  }
  const auto opened = ohl::platform::open_media_source(source);
  auto validation = ohl::media::validate_iso(opened.source);
  if (!opened.valid() || !validation.valid()) {
    std::cerr << "failed to validate synthetic import source\n";
    return 1;
  }
  const auto fingerprint = validation.media->fingerprint();

  if (ohl::media::detail::verify_complete_source_stability(
          *validation.media) != ohl::media::detail::SourceStabilityError::none) {
    std::cerr << "stable validated source failed complete verification\n";
    return 1;
  }

  ohl::media::CancellationSource cancelled_source;
  (void)cancelled_source.request_stop();
  const auto cancelled_cache_root = temporary.path() / "cancelled-cache";
  if (ohl::media::detail::verify_complete_source_stability(
          *validation.media, cancelled_source.get_token()) !=
          ohl::media::detail::SourceStabilityError::cancelled ||
      std::filesystem::exists(cancelled_cache_root)) {
    std::cerr << "pre-requested complete verification was not cancelled\n";
    return 1;
  }

  const auto moved_source = temporary.path() / "moved-capability.iso";
  if (!write_image(moved_source, make_valid_image())) {
    std::cerr << "failed to create moved-capability fixture\n";
    return 1;
  }
  const auto moved_opened = ohl::platform::open_media_source(moved_source);
  auto moved_validation = ohl::media::validate_iso(moved_opened.source);
  if (!moved_opened.valid() || !moved_validation.valid()) {
    std::cerr << "failed to validate moved-capability fixture\n";
    return 1;
  }
  auto retained_media = std::move(*moved_validation.media);
  const auto moved_cache_root = temporary.path() / "moved-cache";
  const auto moved_verification =
      ohl::media::detail::verify_complete_source_stability(
          *moved_validation.media);
  const auto moved_import = ohl::media::prepare_import_cache(
      *moved_validation.media, moved_cache_root);
  if (!retained_media.valid() ||
      moved_verification !=
          ohl::media::detail::SourceStabilityError::invalid_capability ||
      moved_import.error != ohl::media::ImportCacheError::invalid_request ||
      std::filesystem::exists(moved_cache_root)) {
    std::cerr << "moved-from capability was accepted or published a manifest\n";
    return 1;
  }

  // Replacing the pathname after validation must not retarget capability cache
  // preparation or its digest revalidation of the pinned source.
  const auto displaced = temporary.path() / "displaced-source.iso";
  std::error_code error_code;
  std::filesystem::rename(source, displaced, error_code);
  if (error_code ||
      !write_image(source,
                   std::vector<std::byte>(kSectorCount * kSectorSize))) {
    std::cerr << "failed to install pathname replacement fixture\n";
    return 1;
  }

  const auto cache_root = temporary.path() / "cache";
  const auto first =
      ohl::media::prepare_import_cache(*validation.media, cache_root);
  if (!first.valid() || first.cache_hit ||
      first.source_sha256 != fingerprint.sha256 ||
      first.source_directory !=
          cache_root / "sources" / fingerprint.sha256) {
    std::cerr << "new cache entry did not preserve validated fingerprint: "
              << ohl::media::to_string(first.error) << '\n';
    return 1;
  }

  std::ifstream manifest_input{first.source_directory / "provenance.json"};
  std::ostringstream manifest_stream;
  manifest_stream << manifest_input.rdbuf();
  const auto manifest = manifest_stream.str();
  if (!manifest_input ||
      manifest.find(fingerprint.sha256) == std::string::npos ||
      manifest.find(std::to_string(fingerprint.size_bytes)) ==
          std::string::npos ||
      manifest.find("CACHE\\\"TEST") == std::string::npos ||
      manifest.find("private-source-name") != std::string::npos ||
      manifest.find("not-imported") == std::string::npos) {
    std::cerr << "provenance manifest was inconsistent or exposed the path\n";
    return 1;
  }
  manifest_input.close();

  const auto manifest_path = first.source_directory / "provenance.json";
  const auto initial_manifest_write_time =
      std::filesystem::last_write_time(manifest_path, error_code);
  if (error_code) {
    std::cerr << "failed to inspect regular provenance manifest\n";
    return 1;
  }
  std::filesystem::last_write_time(
      manifest_path, initial_manifest_write_time - std::chrono::hours{24},
      error_code);
  const auto cache_hit_write_time =
      std::filesystem::last_write_time(manifest_path, error_code);
  if (error_code) {
    std::cerr << "failed to mark regular provenance manifest\n";
    return 1;
  }

  const auto second =
      ohl::media::prepare_import_cache(*validation.media, cache_root);
  const auto cache_hit_result_write_time =
      std::filesystem::last_write_time(manifest_path, error_code);
  if (error_code) {
    std::cerr << "cache hit removed the regular provenance manifest\n";
    return 1;
  }
  const auto manifest_status =
      std::filesystem::symlink_status(manifest_path, error_code);
  if (error_code || !std::filesystem::is_regular_file(manifest_status) ||
      std::filesystem::is_symlink(manifest_status) || !second.valid() ||
      !second.cache_hit ||
      second.source_sha256 != fingerprint.sha256 ||
      cache_hit_result_write_time != cache_hit_write_time) {
    std::cerr << "matching regular no-follow provenance cache was not reused\n";
    return 1;
  }

  if (!write_text(first.source_directory / "provenance.json", "tampered\n")) {
    std::cerr << "failed to replace test provenance manifest\n";
    return 1;
  }
  const auto conflict =
      ohl::media::prepare_import_cache(*validation.media, cache_root);
  std::string conflicting_manifest;
  if (conflict.error != ohl::media::ImportCacheError::manifest_conflict ||
      !read_text(first.source_directory / "provenance.json",
                 conflicting_manifest) ||
      conflicting_manifest != "tampered\n") {
    std::cerr << "conflicting provenance manifest was overwritten\n";
    return 1;
  }

  const auto publication_race_root = temporary.path() / "publication-race";
  PublishRaceContext publication_race{
      .destination = publication_race_root / "sources" / fingerprint.sha256 /
                     "provenance.json",
      .conflicting_contents = "late publication conflict\n",
  };
  const auto raced = ohl::media::detail::prepare_import_cache_with_hook(
      *validation.media, publication_race_root,
      {.before_publish = create_publish_conflict,
       .context = &publication_race});
  std::string raced_manifest;
  if (!publication_race.invoked || !publication_race.conflict_created ||
      raced.error != ohl::media::ImportCacheError::manifest_conflict ||
      !read_text(publication_race.destination, raced_manifest) ||
      raced_manifest != publication_race.conflicting_contents) {
    std::cerr << "publication race replaced the conflicting manifest\n";
    return 1;
  }

  const auto linked_manifest_root = temporary.path() / "linked-manifest-cache";
  const auto linked_manifest_directory =
      linked_manifest_root / "sources" / fingerprint.sha256;
  const auto linked_manifest_target =
      temporary.path() / "linked-manifest-target.json";
  std::filesystem::create_directories(linked_manifest_directory, error_code);
  if (error_code || !write_text(linked_manifest_target, manifest)) {
    std::cerr << "failed to create linked-manifest rejection fixture\n";
    return 1;
  }
  const auto linked_manifest =
      linked_manifest_directory / "provenance.json";
  std::filesystem::create_symlink(linked_manifest_target, linked_manifest,
                                  error_code);
#if !defined(_WIN32)
  if (error_code) {
    std::cerr << "failed to create linked-manifest rejection fixture\n";
    return 1;
  }
#endif
  if (!error_code) {
    const auto linked_destination = ohl::media::prepare_import_cache(
        *validation.media, linked_manifest_root);
    std::string linked_target_contents;
    if (linked_destination.error !=
            ohl::media::ImportCacheError::unsafe_cache_path ||
        !read_text(linked_manifest_target, linked_target_contents) ||
        linked_target_contents != manifest) {
      std::cerr << "linked provenance destination was followed or overwritten\n";
      return 1;
    }
  }

  const auto relative = ohl::media::prepare_import_cache(
      *validation.media, std::filesystem::path{"relative-cache"});
  if (relative.error != ohl::media::ImportCacheError::invalid_request) {
    std::cerr << "relative cache root was not rejected\n";
    return 1;
  }

  const auto link = temporary.path() / "linked-cache";
  std::filesystem::create_directory_symlink(cache_root, link, error_code);
#if !defined(_WIN32)
  if (error_code) {
    std::cerr << "failed to create linked-cache rejection fixture\n";
    return 1;
  }
#endif
  if (!error_code) {
    const auto linked =
        ohl::media::prepare_import_cache(*validation.media, link);
    if (linked.error != ohl::media::ImportCacheError::unsafe_cache_path) {
      std::cerr << "linked cache root was not rejected\n";
      return 1;
    }
  }

  const auto mutable_source = temporary.path() / "mutable-source.iso";
  if (!write_image(mutable_source, make_valid_image())) {
    std::cerr << "failed to create mutable source fixture\n";
    return 1;
  }
  const auto mutable_opened =
      ohl::platform::open_media_source(mutable_source);
  auto mutable_validation =
      ohl::media::validate_iso(mutable_opened.source);
  const auto original_write_time =
      std::filesystem::last_write_time(mutable_source, error_code);
  auto changed_image = make_valid_image();
  changed_image[0] ^= std::byte{1};
  if (!mutable_opened.valid() || !mutable_validation.valid() || error_code ||
      !write_image(mutable_source, changed_image)) {
    std::cerr << "failed to prepare same-object cache mutation\n";
    return 1;
  }
  std::filesystem::last_write_time(
      mutable_source, original_write_time - std::chrono::seconds{1},
      error_code);
  if (error_code) {
    std::cerr << "failed to make cache mutation metadata observable\n";
    return 1;
  }
  ohl::media::CancellationSource changed_stop_source;
  (void)changed_stop_source.request_stop();
  const auto changed_cache_root = temporary.path() / "changed-cache";
  const auto changed_manifest =
      changed_cache_root / "sources" /
      mutable_validation.media->fingerprint().sha256 / "provenance.json";
  if (ohl::media::detail::verify_complete_source_stability(
          *mutable_validation.media, changed_stop_source.get_token()) !=
          ohl::media::detail::SourceStabilityError::source_changed ||
      std::filesystem::exists(changed_manifest, error_code) || error_code) {
    std::cerr << "source change did not take precedence over cancellation\n";
    return 1;
  }
  const auto changed = ohl::media::prepare_import_cache(
      *mutable_validation.media, changed_cache_root);
  error_code.clear();
  if (changed.error != ohl::media::ImportCacheError::source_changed ||
      std::filesystem::exists(changed_manifest, error_code) || error_code) {
    std::cerr << "metadata revalidation failure published a manifest\n";
    return 1;
  }

  const auto digest_source = temporary.path() / "digest-mutated-source.iso";
  if (!write_image(digest_source, make_valid_image())) {
    std::cerr << "failed to create digest-mutation fixture\n";
    return 1;
  }
  const auto digest_opened = ohl::platform::open_media_source(digest_source);
  auto digest_validation = ohl::media::validate_iso(digest_opened.source);
  const auto digest_write_time =
      std::filesystem::last_write_time(digest_source, error_code);
  auto digest_changed_image = make_valid_image();
  digest_changed_image[0] ^= std::byte{1};
  if (!digest_opened.valid() || !digest_validation.valid() || error_code ||
      !write_image(digest_source, digest_changed_image)) {
    std::cerr << "failed to mutate digest fixture without changing size\n";
    return 1;
  }
  std::filesystem::last_write_time(digest_source, digest_write_time,
                                   error_code);
  if (error_code ||
      digest_opened.source->verify_unchanged() !=
          ohl::platform::MediaSourceError::none) {
    std::cerr << "failed to restore observable digest fixture metadata\n";
    return 1;
  }

  const auto digest_cache_root = temporary.path() / "digest-changed-cache";
  const auto digest_manifest =
      digest_cache_root / "sources" /
      digest_validation.media->fingerprint().sha256 / "provenance.json";
  error_code.clear();
  if (ohl::media::detail::verify_complete_source_stability(
          *digest_validation.media) !=
          ohl::media::detail::SourceStabilityError::digest_mismatch ||
      std::filesystem::exists(digest_manifest, error_code) || error_code) {
    std::cerr << "same-size restored-metadata mutation was not a digest mismatch\n";
    return 1;
  }
  const auto digest_changed = ohl::media::prepare_import_cache(
      *digest_validation.media, digest_cache_root);
  error_code.clear();
  if (digest_changed.error != ohl::media::ImportCacheError::source_changed ||
      std::filesystem::exists(digest_manifest, error_code) || error_code) {
    std::cerr << "digest mismatch was accepted or published a manifest\n";
    return 1;
  }

  return 0;
}
