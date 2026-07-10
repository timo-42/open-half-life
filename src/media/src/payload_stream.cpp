#include "ohl/media/payload_stream.hpp"

#include <utility>

namespace ohl::media {
namespace {

class BoundedPayloadSink final : public PayloadByteSink {
 public:
  BoundedPayloadSink(const std::uint64_t declared_size,
                     PayloadByteSink& destination) noexcept
      : declared_size_(declared_size), destination_(destination) {}

  [[nodiscard]] bool write(
      const std::span<const std::byte> bytes) noexcept override {
    if (error_ != PayloadStreamError::none) {
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
  PayloadByteSink& destination_;
  std::uint64_t bytes_written_{0};
  PayloadStreamError error_{PayloadStreamError::none};
};

}  // namespace

PayloadStreamResult stream_payload_entry(
    const PlannedPayloadEntry& entry, PayloadSource& source,
    PayloadByteSink& destination) noexcept {
  BoundedPayloadSink bounded_sink(entry.size_bytes, destination);
  const auto source_succeeded =
      source.stream(entry.source_token, bounded_sink);

  if (bounded_sink.error() != PayloadStreamError::none) {
    return {.error = bounded_sink.error(),
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
