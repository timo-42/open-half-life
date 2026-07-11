#pragma once

#include "ohl/media/payload_layout.hpp"
#include "ohl/platform/media_source.hpp"

#include <cstddef>
#include <cstdint>
#include <span>
#include <stop_token>

namespace ohl::media {

// Writes are synchronous and must not throw. A destination must return true
// only after accepting the entire chunk and must handle any OS-level partial
// writes internally. Returning false may leave partial staging side effects;
// callers must discard destination staging after an unsuccessful stream.
class PayloadByteSink {
 public:
  virtual ~PayloadByteSink() = default;

  [[nodiscard]] virtual bool write(
      std::span<const std::byte> bytes) noexcept = 0;
};

// Streaming is synchronous and must not throw. Sources receive the exact pinned
// media capability supplied to staging plus only the opaque token produced
// during layout planning. They must cooperate with cancellation, stop and
// return false when the supplied sink rejects a chunk, and must neither retain
// nor use the source or sink after stream() returns.
class PayloadSource {
 public:
  virtual ~PayloadSource() = default;

  [[nodiscard]] virtual bool stream(
      const platform::MediaSource& media_source, std::uint64_t source_token,
      std::stop_token stop_token, PayloadByteSink& sink) noexcept = 0;
};

enum class PayloadStreamError {
  none,
  source_failure,
  destination_failure,
  overflow,
  underflow,
  cancelled,
};

struct PayloadStreamResult {
  PayloadStreamError error{PayloadStreamError::none};
  // Counts only chunks that the destination accepted in full.
  std::uint64_t bytes_written{0};

  [[nodiscard]] bool complete() const noexcept {
    return error == PayloadStreamError::none;
  }
};

// Streams one planned entry without exposing its destination path to the
// source. The destination is never called with a chunk that would exceed the
// entry's declared size, and success requires an exact final byte count.
[[nodiscard]] PayloadStreamResult stream_payload_entry(
    const PlannedPayloadEntry& entry,
    const platform::MediaSource& media_source, PayloadSource& source,
    std::stop_token stop_token,
    PayloadByteSink& destination) noexcept;

}  // namespace ohl::media
