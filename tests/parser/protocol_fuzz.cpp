#include "ohl/parser/protocol.hpp"

#include <cstddef>
#include <cstdint>
#include <span>

extern "C" int LLVMFuzzerTestOneInput(const std::uint8_t* data,
                                       const std::size_t size) {
  if (data == nullptr) {
    return 0;
  }
  const auto bytes = std::as_bytes(std::span{data, size});
  const auto frame = ohl::parser::decode_frame(bytes);
  if (!frame.valid()) {
    return 0;
  }

  ohl::parser::PayloadReader reader{frame.payload};
  while (reader.remaining() >= sizeof(std::uint64_t)) {
    std::uint64_t value = 0;
    if (!reader.read_u64(value)) {
      break;
    }
  }
  (void)reader.finish();

  ohl::parser::ProtocolStateValidator validator{frame.header.session_id};
  const auto direction = frame.payload.empty() ||
                                 (std::to_integer<std::uint8_t>(
                                      frame.payload.front()) &
                                  1U) == 0
                             ? ohl::parser::MessageDirection::parent_to_worker
                             : ohl::parser::MessageDirection::worker_to_parent;
  (void)validator.observe(direction, frame.header);
  return 0;
}
