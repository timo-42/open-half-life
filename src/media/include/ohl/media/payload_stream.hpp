#pragma once

#include "ohl/media/payload_layout.hpp"

#include <cstddef>
#include <cstdint>
#include <span>

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

// Streaming is synchronous and must not throw. Sources receive only the opaque
// token produced during layout planning, must stop and return false when the
// supplied sink rejects a chunk, and must neither retain nor use the sink after
// stream() returns.
class PayloadSource {
 public:
  virtual ~PayloadSource() = default;

  [[nodiscard]] virtual bool stream(
      std::uint64_t source_token, PayloadByteSink& sink) noexcept = 0;
};

enum class PayloadStreamError {
  none,
  source_failure,
  destination_failure,
  overflow,
  underflow,
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
    const PlannedPayloadEntry& entry, PayloadSource& source,
    PayloadByteSink& destination) noexcept;

}  // namespace ohl::media
