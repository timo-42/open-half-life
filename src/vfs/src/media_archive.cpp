#include "ohl/vfs/media_archive.hpp"

#include <utility>

namespace ohl::vfs {
namespace {

template <typename Handle>
class MediaFileAdapter final : public MediaFile {
 public:
  explicit MediaFileAdapter(std::unique_ptr<Handle> file) noexcept
      : file_{std::move(file)} {}

  [[nodiscard]] std::uint64_t size() const noexcept override {
    return file_ == nullptr ? 0 : file_->size();
  }

  [[nodiscard]] std::int64_t tell() const noexcept override {
    return file_ == nullptr ? -1 : file_->tell();
  }

  [[nodiscard]] std::int64_t read(
      const std::span<std::byte> destination) override {
    return file_ == nullptr ? -1 : file_->read(destination);
  }

  [[nodiscard]] bool seek(const std::uint64_t offset) override {
    return file_ != nullptr && file_->seek(offset);
  }

 private:
  std::unique_ptr<Handle> file_;
};

template <typename Handle>
[[nodiscard]] std::unique_ptr<MediaFile> adapt(
    std::unique_ptr<Handle> file) {
  if (file == nullptr) {
    return nullptr;
  }
  return std::make_unique<MediaFileAdapter<Handle>>(std::move(file));
}

}  // namespace

MediaFile::~MediaFile() = default;

struct MediaArchive::Impl {
  MediaFormat format{MediaFormat::udf};
  UdfArchive udf;
  Iso9660Archive iso9660;
};

MediaArchive::MediaArchive() : implementation_{std::make_unique<Impl>()} {}
MediaArchive::~MediaArchive() = default;
MediaArchive::MediaArchive(MediaArchive&&) noexcept = default;
MediaArchive& MediaArchive::operator=(MediaArchive&&) noexcept = default;

VfsError MediaArchive::open(const MediaFormat format,
                            SharedMediaSource source,
                            const UdfArchiveLimits limits) {
  if (implementation_ == nullptr) {
    implementation_ = std::make_unique<Impl>();
  }
  close();
  implementation_->format = format;
  return format == MediaFormat::iso9660
             ? implementation_->iso9660.open(std::move(source), limits)
             : implementation_->udf.open(std::move(source), limits);
}

MediaArchive MediaArchive::share() const {
  MediaArchive result;
  if (implementation_ == nullptr) {
    return result;
  }
  result.implementation_->format = implementation_->format;
  if (implementation_->format == MediaFormat::iso9660) {
    result.implementation_->iso9660 = implementation_->iso9660.share();
  } else {
    result.implementation_->udf = implementation_->udf.share();
  }
  return result;
}

void MediaArchive::close() noexcept {
  if (implementation_ == nullptr) {
    return;
  }
  implementation_->udf.close();
  implementation_->iso9660.close();
}

bool MediaArchive::is_open() const noexcept {
  if (implementation_ == nullptr) {
    return false;
  }
  return implementation_->format == MediaFormat::iso9660
             ? implementation_->iso9660.is_open()
             : implementation_->udf.is_open();
}

MediaFormat MediaArchive::format() const noexcept {
  return implementation_ == nullptr ? MediaFormat::udf
                                    : implementation_->format;
}

std::string MediaArchive::volume_label() const {
  if (implementation_ == nullptr) {
    return {};
  }
  return implementation_->format == MediaFormat::iso9660
             ? implementation_->iso9660.volume_label()
             : implementation_->udf.volume_label();
}

DirectoryPage MediaArchive::list_page(const std::string_view path) const {
  if (implementation_ == nullptr) {
    DirectoryPage result;
    result.error = VfsError::not_open;
    return result;
  }
  return implementation_->format == MediaFormat::iso9660
             ? implementation_->iso9660.list_page(path)
             : implementation_->udf.list_page(path);
}

DirectoryPage MediaArchive::continue_list(DirectoryCursor cursor) const {
  if (implementation_ == nullptr) {
    DirectoryPage result;
    result.error = VfsError::not_open;
    return result;
  }
  return implementation_->format == MediaFormat::iso9660
             ? implementation_->iso9660.continue_list(std::move(cursor))
             : implementation_->udf.continue_list(std::move(cursor));
}

DirectoryListing MediaArchive::list(const std::string_view path) const {
  if (implementation_ == nullptr) {
    DirectoryListing result;
    result.error = VfsError::not_open;
    return result;
  }
  return implementation_->format == MediaFormat::iso9660
             ? implementation_->iso9660.list(path)
             : implementation_->udf.list(path);
}

std::unique_ptr<MediaFile> MediaArchive::open_file(
    const std::string_view path) const {
  if (implementation_ == nullptr) {
    return nullptr;
  }
  return implementation_->format == MediaFormat::iso9660
             ? adapt(implementation_->iso9660.open_file(path))
             : adapt(implementation_->udf.open_file(path));
}

std::unique_ptr<MediaFile> MediaArchive::open_file_at(
    const std::string_view directory, const std::string_view entry_name) const {
  if (implementation_ == nullptr) {
    return nullptr;
  }
  return implementation_->format == MediaFormat::iso9660
             ? adapt(implementation_->iso9660.open_file_at(directory,
                                                           entry_name))
             : adapt(implementation_->udf.open_file_at(directory, entry_name));
}

}  // namespace ohl::vfs
