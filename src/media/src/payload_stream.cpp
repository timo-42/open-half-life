#include "ohl/media/payload_stream.hpp"

#include <utility>

namespace ohl::media {
namespace {

class BoundedPayloadSink final : public PayloadByteSink {
 public:
  BoundedPayloadSink(const std::uint64_t declared_size,
                     const std::stop_token stop_token,
                     PayloadByteSink& destination) noexcept
      : declared_size_(declared_size),
        stop_token_(stop_token),
        destination_(destination) {}

  [[nodiscard]] bool write(
      const std::span<const std::byte> bytes) noexcept override {
    if (error_ != PayloadStreamError::none) {
      return false;
    }
    if (stop_token_.stop_requested()) {
      error_ = PayloadStreamError::cancelled;
      return false;
    }

    const auto remaining = declared_size_ - bytes_written_;
    if (std::cmp_greater(bytes.size(), remaining)) {
      error_ = PayloadStreamError::overflow;
      return false;
    }
    if (!destination_.write(bytes)) {
      error_ = PayloadStreamError::destination_failure;
      return false;
    }

    bytes_written_ += static_cast<std::uint64_t>(bytes.size());
    return true;
  }

  [[nodiscard]] PayloadStreamError error() const noexcept { return error_; }
  [[nodiscard]] std::uint64_t bytes_written() const noexcept {
    return bytes_written_;
  }

 private:
  std::uint64_t declared_size_{0};
  std::stop_token stop_token_;
  PayloadByteSink& destination_;
  std::uint64_t bytes_written_{0};
  PayloadStreamError error_{PayloadStreamError::none};
};

}  // namespace

PayloadStreamResult stream_payload_entry(
    const PlannedPayloadEntry& entry,
    const platform::MediaSource& media_source, PayloadSource& source,
    const std::stop_token stop_token,
    PayloadByteSink& destination) noexcept {
  if (stop_token.stop_requested()) {
    return {.error = PayloadStreamError::cancelled};
  }

  BoundedPayloadSink bounded_sink(entry.size_bytes, stop_token, destination);
  const auto source_succeeded = source.stream(
      media_source, entry.source_token, stop_token, bounded_sink);

  if (bounded_sink.error() != PayloadStreamError::none) {
    return {.error = bounded_sink.error(),
            .bytes_written = bounded_sink.bytes_written()};
  }
  if (stop_token.stop_requested()) {
    return {.error = PayloadStreamError::cancelled,
            .bytes_written = bounded_sink.bytes_written()};
  }
  if (!source_succeeded) {
    return {.error = PayloadStreamError::source_failure,
            .bytes_written = bounded_sink.bytes_written()};
  }
  if (bounded_sink.bytes_written() != entry.size_bytes) {
    return {.error = PayloadStreamError::underflow,
            .bytes_written = bounded_sink.bytes_written()};
  }
  return {.bytes_written = bounded_sink.bytes_written()};
}

}  // namespace ohl::media
