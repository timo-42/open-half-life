#include "udf_media_input.hpp"

#include <udfread/blockinput.h>

#include <atomic>
#include <cstddef>
#include <limits>
#include <span>
#include <utility>

namespace ohl::vfs::detail {
namespace {

using platform::MediaSourceError;

struct MediaBlockInput final : udfread_block_input {
  MediaBlockInput(SharedMediaSource input_source,
                  const std::uint32_t input_block_count,
                  const std::uint32_t input_max_blocks_per_read)
      : source{std::move(input_source)},
        block_count{input_block_count},
        max_blocks_per_read{input_max_blocks_per_read} {
    close = &close_callback;
    read = &read_callback;
    size = &size_callback;
  }

  static int close_callback(udfread_block_input* input) noexcept {
    delete static_cast<MediaBlockInput*>(input);
    return 0;
  }

  static int read_callback(udfread_block_input* input, const std::uint32_t lba,
                           void* const destination,
                           const std::uint32_t requested_blocks,
                           const int flags) noexcept {
    (void)flags;
    return udf_media_input_read_blocks(input, lba, destination,
                                       requested_blocks);
  }

  static std::uint32_t size_callback(udfread_block_input* input) noexcept {
    return udf_media_input_size_blocks(input);
  }

  void record_error(const MediaSourceError error) noexcept {
    source_error.store(error);
  }

  SharedMediaSource source;
  std::uint32_t block_count{0};
  std::uint32_t max_blocks_per_read{0};
  std::atomic<MediaSourceError> source_error{MediaSourceError::none};
};

[[nodiscard]] MediaBlockInput* as_media_input(
    udfread_block_input* const input) noexcept {
  return static_cast<MediaBlockInput*>(input);
}

[[nodiscard]] const MediaBlockInput* as_media_input(
    const udfread_block_input* const input) noexcept {
  return static_cast<const MediaBlockInput*>(input);
}

}  // namespace

void UdfMediaInputDeleter::operator()(
    udfread_block_input* const input) const noexcept {
  if (input != nullptr) {
    (void)MediaBlockInput::close_callback(input);
  }
}

VfsError validate_udf_media_source_size(
    const std::uint64_t source_size, const UdfArchiveLimits& limits,
    std::uint32_t& block_count) noexcept {
  block_count = 0;
  if (source_size == 0 ||
      source_size % UdfArchiveLimits::logical_block_size != 0) {
    return VfsError::invalid_source;
  }
  if (source_size > limits.max_source_bytes ||
      source_size > UdfArchiveLimits::max_representable_source_bytes) {
    return VfsError::limit_exceeded;
  }

  const auto blocks = source_size / UdfArchiveLimits::logical_block_size;
  if (blocks > std::numeric_limits<std::uint32_t>::max()) {
    return VfsError::limit_exceeded;
  }
  block_count = static_cast<std::uint32_t>(blocks);
  return VfsError::none;
}

UdfMediaInputCreateResult create_udf_media_input(
  SharedMediaSource source, const UdfArchiveLimits& limits) {
  if (source == nullptr) {
    return {.input = {}, .error = VfsError::invalid_source};
  }
  if (limits.max_blocks_per_read == 0 ||
      limits.max_blocks_per_read >
          static_cast<std::uint32_t>(std::numeric_limits<int>::max())) {
    return {.input = {}, .error = VfsError::limit_exceeded};
  }

  const auto stable = source->verify_unchanged();
  if (stable != MediaSourceError::none) {
    return {
        .input = {},
        .error = stable == MediaSourceError::changed
                     ? VfsError::source_changed
                     : VfsError::read_failed,
    };
  }

  std::uint32_t block_count = 0;
  const auto size_error =
      validate_udf_media_source_size(source->size(), limits, block_count);
  if (size_error != VfsError::none) {
    return {.input = {}, .error = size_error};
  }

  return {
      .input = UdfMediaInputHandle{new MediaBlockInput{
          std::move(source), block_count, limits.max_blocks_per_read}},
      .error = VfsError::none,
  };
}

int udf_media_input_read_blocks(udfread_block_input* const input,
                                const std::uint32_t lba,
                                void* const destination,
                                const std::uint32_t requested_blocks) noexcept {
  if (input == nullptr) {
    return -1;
  }
  auto* const media_input = as_media_input(input);
  if (requested_blocks == 0) {
    return 0;
  }
  if (destination == nullptr ||
      requested_blocks > media_input->max_blocks_per_read ||
      requested_blocks >
          static_cast<std::uint32_t>(std::numeric_limits<int>::max())) {
    media_input->record_error(MediaSourceError::out_of_range);
    return -1;
  }

  const auto first_block = static_cast<std::uint64_t>(lba);
  const auto blocks = static_cast<std::uint64_t>(requested_blocks);
  if (first_block > media_input->block_count ||
      blocks > static_cast<std::uint64_t>(media_input->block_count) -
                   first_block) {
    media_input->record_error(MediaSourceError::out_of_range);
    return -1;
  }

  constexpr auto block_size = UdfArchiveLimits::logical_block_size;
  if (blocks > std::numeric_limits<std::uint64_t>::max() / block_size ||
      first_block > std::numeric_limits<std::uint64_t>::max() / block_size) {
    media_input->record_error(MediaSourceError::out_of_range);
    return -1;
  }
  const auto bytes = blocks * block_size;
  const auto offset = first_block * block_size;
  if (bytes > std::numeric_limits<std::size_t>::max() ||
      offset > std::numeric_limits<std::uint64_t>::max() - bytes) {
    media_input->record_error(MediaSourceError::resource_exhausted);
    return -1;
  }

  auto error = media_input->source->verify_unchanged();
  if (error == MediaSourceError::none) {
    const auto read_error = media_input->source->read_exact_at(
        offset, std::span{static_cast<std::byte*>(destination),
                          static_cast<std::size_t>(bytes)});
    const auto final_error = media_input->source->verify_unchanged();
    error = final_error == MediaSourceError::none ? read_error : final_error;
  }
  if (error != MediaSourceError::none) {
    media_input->record_error(error);
    return -1;
  }
  media_input->source_error.store(MediaSourceError::none);
  return static_cast<int>(requested_blocks);
}

std::uint32_t udf_media_input_size_blocks(
    udfread_block_input* const input) noexcept {
  return input == nullptr ? 0 : as_media_input(input)->block_count;
}

MediaSourceError udf_media_input_source_error(
    const udfread_block_input* const input) noexcept {
  return input == nullptr ? MediaSourceError::read_failed
                          : as_media_input(input)->source_error.load();
}

}  // namespace ohl::vfs::detail
