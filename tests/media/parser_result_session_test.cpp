#include "ohl/media/parser_result_session.hpp"

#include <algorithm>
#include <array>
#include <cstddef>
#include <cstdint>
#include <cstdlib>
#include <iostream>
#include <limits>
#include <new>
#include <span>
#include <string_view>
#include <utility>
#include <vector>

namespace allocation_injection {

#if defined(__GNUC__) && !defined(__clang__)
#define OHL_TEST_NO_IPA __attribute__((noinline, noipa))
#elif defined(__clang__)
#define OHL_TEST_NO_IPA __attribute__((noinline))
#elif defined(_MSC_VER)
#define OHL_TEST_NO_IPA __declspec(noinline)
#else
#define OHL_TEST_NO_IPA
#endif

enum class Mode { disabled, bad_alloc, other_exception };

struct SyntheticException {};

Mode mode = Mode::disabled;

void fail_next(const Mode requested) noexcept { mode = requested; }

void maybe_fail() {
  const auto requested = std::exchange(mode, Mode::disabled);
  if (requested == Mode::bad_alloc) {
    throw std::bad_alloc{};
  }
  if (requested == Mode::other_exception) {
    throw SyntheticException{};
  }
}

// Keep the raw C allocation family behind a non-IPA boundary. GCC otherwise
// attributes the inlined free() to a sized replacement delete and diagnoses a
// mismatched allocation family even though every replacement new below uses
// the matching raw allocator.
OHL_TEST_NO_IPA void* allocate(const std::size_t size) {
  maybe_fail();
  if (auto* allocation = std::malloc(size == 0 ? 1 : size)) {
    return allocation;
  }
  throw std::bad_alloc{};
}

OHL_TEST_NO_IPA void deallocate(void* const allocation) noexcept {
  std::free(allocation);
}

#undef OHL_TEST_NO_IPA

}  // namespace allocation_injection

void* operator new(const std::size_t size) {
  return allocation_injection::allocate(size);
}

void* operator new[](const std::size_t size) {
  return allocation_injection::allocate(size);
}

void operator delete(void* allocation) noexcept {
  allocation_injection::deallocate(allocation);
}
void operator delete[](void* allocation) noexcept {
  allocation_injection::deallocate(allocation);
}
void operator delete(void* allocation, std::size_t) noexcept {
  allocation_injection::deallocate(allocation);
}
void operator delete[](void* allocation, std::size_t) noexcept {
  allocation_injection::deallocate(allocation);
}

namespace {

using ohl::media::ParserResultSession;
using ohl::media::ParserResultSessionError;
using ohl::media::ParserResultCatalogGeneration;
using ohl::media::ParserReadRequestDisposition;
using ohl::media::PayloadByteSink;
using ohl::media::PayloadImportLimits;
using ohl::parser::FrameHeader;
using ohl::parser::FrameView;
using ohl::parser::MessageDirection;
using ohl::parser::MessageType;
using ohl::parser::PayloadWriter;
using ohl::parser::ProtocolError;
using ohl::parser::ProtocolPhase;
using ohl::parser::ProtocolStateValidator;
using ohl::parser::ProtocolStatus;
using ohl::parser::SessionState;

constexpr std::uint64_t kSession = 0x0102'0304'0506'0708ULL;

[[nodiscard]] std::uint64_t next_worker_epoch() noexcept {
  static std::uint64_t epoch = 0x4000'0000'0000'0001ULL;
  return epoch++;
}

[[nodiscard]] bool fail(const std::string_view message) {
  std::cerr << message << '\n';
  return false;
}

[[nodiscard]] FrameHeader header(const MessageType type,
                                 const std::uint64_t request_id,
                                 const std::size_t payload_size) {
  return {
      .type = type,
      .payload_length = static_cast<std::uint32_t>(payload_size),
      .session_id = kSession,
      .request_id = request_id,
  };
}

[[nodiscard]] FrameView frame(const MessageType type,
                              const std::span<const std::byte> payload,
                              const std::uint64_t request_id) {
  return {
      .header = header(type, request_id, payload.size()),
      .payload = payload,
  };
}

[[nodiscard]] ProtocolStateValidator idle_protocol() {
  ProtocolStateValidator protocol{kSession};
  if (protocol.observe(MessageDirection::parent_to_worker,
                       header(MessageType::hello, 0,
                              ohl::parser::kHelloPayloadBytes)) !=
          ProtocolError::none ||
      protocol.observe(MessageDirection::worker_to_parent,
                       header(MessageType::ready, 0, 0)) !=
          ProtocolError::none) {
    std::abort();
  }
  return protocol;
}

struct SyntheticEntry {
  std::uint64_t token{0};
  std::uint64_t size{0};
  std::string_view path;
};

[[nodiscard]] std::vector<std::byte> entry_batch_payload(
    const std::span<const SyntheticEntry> entries) {
  std::size_t size = ohl::parser::kEntryBatchPrefixBytes;
  for (const auto& entry : entries) {
    size += ohl::parser::kEntryBatchEntryPrefixBytes + entry.path.size();
  }
  std::vector<std::byte> payload(size);
  PayloadWriter writer{payload};
  if (!writer.write_u16(static_cast<std::uint16_t>(entries.size()))) {
    std::abort();
  }
  for (const auto& entry : entries) {
    if (!writer.write_u64(entry.token) || !writer.write_u64(entry.size) ||
        !writer.write_u16(static_cast<std::uint16_t>(entry.path.size())) ||
        !writer.write_bytes(std::as_bytes(std::span<const char>{
            entry.path.data(), entry.path.size()}))) {
      std::abort();
    }
  }
  return payload;
}

[[nodiscard]] std::array<std::byte, ohl::parser::kStreamEntryPayloadBytes>
stream_entry_payload(const std::uint64_t token) {
  std::array<std::byte, ohl::parser::kStreamEntryPayloadBytes> payload{};
  const auto encoded = ohl::parser::encode_stream_entry_payload({token}, payload);
  if (!encoded.valid()) {
    std::abort();
  }
  return payload;
}

[[nodiscard]] constexpr std::array<std::byte,
                                   ohl::parser::kCompletePayloadBytes>
complete_payload() {
  return {std::byte{0x00}, std::byte{0x00}, std::byte{0x04}, std::byte{0x00}};
}

[[nodiscard]] bool begin_enumeration(ParserResultSession& session,
                                     const std::uint64_t request_id = 1) {
  return session.begin_enumeration(
                    frame(MessageType::enumerate, {}, request_id))
      .valid();
}

[[nodiscard]] bool accept_batch(
    ParserResultSession& session, const std::span<const SyntheticEntry> entries,
    const std::uint64_t request_id = 1) {
  const auto payload = entry_batch_payload(entries);
  return session.accept_entry_batch(
                    frame(MessageType::entry_batch, payload, request_id))
      .valid();
}

[[nodiscard]] bool complete_enumeration(ParserResultSession& session,
                                        const std::uint64_t request_id = 1) {
  constexpr auto payload = complete_payload();
  return session.complete_enumeration(
                    frame(MessageType::complete, payload, request_id))
      .valid();
}

[[nodiscard]] bool build_catalog(
    ParserResultSession& session, const std::span<const SyntheticEntry> entries,
    const std::uint64_t request_id = 1) {
  return begin_enumeration(session, request_id) &&
         (entries.empty() || accept_batch(session, entries, request_id)) &&
         complete_enumeration(session, request_id);
}

[[nodiscard]] bool begin_stream(ParserResultSession& session,
                                const std::uint64_t token,
                                const ParserResultCatalogGeneration generation,
                                const std::uint64_t request_id) {
  const auto payload = stream_entry_payload(token);
  return session
      .begin_stream_entry(frame(MessageType::stream_entry, payload, request_id),
                          generation)
      .valid();
}

[[nodiscard]] bool complete_stream(ParserResultSession& session,
                                   const std::uint64_t request_id) {
  constexpr auto payload = complete_payload();
  return session.complete_stream(
                    frame(MessageType::complete, payload, request_id))
      .valid();
}

class RecordingSink final : public PayloadByteSink {
 public:
  explicit RecordingSink(const bool accept = true,
                         const ParserResultSession* session = nullptr)
      : accept_{accept}, session_{session} {}

  [[nodiscard]] bool write(
      const std::span<const std::byte> bytes) noexcept override {
    ++calls_;
    observed_remainder_ =
        session_ == nullptr ? 0 : session_->remaining_stream_bytes();
    if (!accept_ || size_ + bytes.size() > bytes_.size()) {
      return false;
    }
    std::copy(bytes.begin(), bytes.end(), bytes_.begin() +
                                             static_cast<std::ptrdiff_t>(size_));
    size_ += bytes.size();
    return true;
  }

  [[nodiscard]] std::size_t calls() const noexcept { return calls_; }
  [[nodiscard]] std::size_t size() const noexcept { return size_; }
  [[nodiscard]] std::uint64_t observed_remainder() const noexcept {
    return observed_remainder_;
  }
  [[nodiscard]] std::span<const std::byte> bytes() const noexcept {
    return std::span<const std::byte>{bytes_}.first(size_);
  }

 private:
  bool accept_{true};
  const ParserResultSession* session_{nullptr};
  std::array<std::byte, ohl::parser::kMaximumDataChunkBytes> bytes_{};
  std::size_t size_{0};
  std::size_t calls_{0};
  std::uint64_t observed_remainder_{0};
};

[[nodiscard]] bool test_catalog_promotion_and_streams() {
  const auto worker_epoch = next_worker_epoch();
  ParserResultSession session{idle_protocol(), worker_epoch};
  if (!begin_enumeration(session)) {
    return fail("enumeration did not begin");
  }
  {
    const std::array first_batch{
        SyntheticEntry{0, 3, "Zulu\\two.bin"},
        SyntheticEntry{2, 0, "Alpha/zero.bin"},
    };
    auto payload = entry_batch_payload(first_batch);
    if (!session.accept_entry_batch(
                    frame(MessageType::entry_batch, payload, 1))
             .valid()) {
      return fail("first catalog batch was rejected");
    }
    std::fill(payload.begin(), payload.end(), std::byte{0xff});
  }
  const std::array second_batch{SyntheticEntry{
      std::numeric_limits<std::uint64_t>::max(), 0, "middle/file.bin"}};
  if (!accept_batch(session, second_batch) || !complete_enumeration(session)) {
    return fail("multi-batch catalog was not promoted");
  }
  const auto catalog = session.catalog();
  if (!catalog.has_value() ||
      catalog->generation != ParserResultCatalogGeneration{worker_epoch, 1} ||
      catalog->entries.size() != 3 || catalog->total_bytes != 3 ||
      catalog->entries[0].relative_path != "Alpha/zero.bin" ||
      catalog->entries[1].relative_path != "middle/file.bin" ||
      catalog->entries[2].relative_path != "Zulu/two.bin") {
    return fail("catalog paths were not copied, normalized, and sorted");
  }

  if (!begin_stream(session, 2, catalog->generation, 2) ||
      session.remaining_stream_bytes() != 0 || !complete_stream(session, 2)) {
    return fail("zero-byte catalog entry did not stream exactly");
  }
  if (!begin_stream(session, 0, catalog->generation, 3) ||
      session.remaining_stream_bytes() != 3) {
    return fail("sorted token index did not resolve zero token");
  }
  const std::array first_chunk{std::byte{0x10}};
  const std::array second_chunk{std::byte{0x20}, std::byte{0x30}};
  RecordingSink sink;
  if (!session.accept_data_chunk(
                   frame(MessageType::data_chunk, first_chunk, 3), sink)
           .valid() ||
      session.remaining_stream_bytes() != 2 ||
      !session.accept_data_chunk(
                   frame(MessageType::data_chunk, second_chunk, 3), sink)
           .valid() ||
      session.remaining_stream_bytes() != 0 || !complete_stream(session, 3) ||
      sink.calls() != 2 || sink.size() != 3 ||
      !std::equal(sink.bytes().begin(), sink.bytes().end(),
                  std::array{std::byte{0x10}, std::byte{0x20},
                             std::byte{0x30}}
                      .begin())) {
    return fail("exact multi-chunk stream failed");
  }
  if (!begin_stream(session, std::numeric_limits<std::uint64_t>::max(),
                    catalog->generation, 4) ||
      session.remaining_stream_bytes() != 0 || !complete_stream(session, 4)) {
    return fail("sorted token index did not resolve maximum token");
  }
  return true;
}

[[nodiscard]] bool test_empty_and_quota_promotions() {
  {
    const auto worker_epoch = next_worker_epoch();
    ParserResultSession empty{idle_protocol(), worker_epoch};
    if (!build_catalog(empty, std::span<const SyntheticEntry>{})) {
      return fail("empty enumeration was not promoted");
    }
    const auto catalog = empty.catalog();
    if (!catalog.has_value() ||
        catalog->generation !=
            ParserResultCatalogGeneration{worker_epoch, 1} ||
        !catalog->entries.empty() || catalog->total_bytes != 0) {
      return fail("empty catalog contents were incorrect");
    }
  }

  const PayloadImportLimits exact_limits{
      .maximum_entries = 2,
      .maximum_path_bytes = 2,
      .maximum_entry_bytes = 6,
      .maximum_total_bytes = 10,
  };
  const std::array exact_entries{SyntheticEntry{1, 4, "a"},
                                 SyntheticEntry{2, 6, "b"}};
  ParserResultSession exact{idle_protocol(), next_worker_epoch(), exact_limits};
  if (!build_catalog(exact, exact_entries) ||
      !exact.catalog().has_value() || exact.catalog()->total_bytes != 10) {
    return fail("exact bridge quotas were rejected");
  }

  const auto rejects_batch = [](const PayloadImportLimits& limits,
                                const std::span<const SyntheticEntry> entries) {
    ParserResultSession session{idle_protocol(), next_worker_epoch(), limits};
    if (!begin_enumeration(session)) {
      return false;
    }
    const auto payload = entry_batch_payload(entries);
    const auto result = session.accept_entry_batch(
        frame(MessageType::entry_batch, payload, 1));
    return result.error == ParserResultSessionError::protocol_failure &&
           result.protocol_error == ProtocolError::noncanonical_value &&
           session.terminal() && !session.catalog().has_value() &&
           session.protocol_state() == SessionState::enumerating;
  };
  const std::array three_entries{SyntheticEntry{1, 1, "a"},
                                 SyntheticEntry{2, 1, "b"},
                                 SyntheticEntry{3, 1, "c"}};
  const std::array long_paths{SyntheticEntry{1, 1, "aa"},
                              SyntheticEntry{2, 1, "bb"}};
  const std::array large_entry{SyntheticEntry{1, 7, "a"}};
  const std::array large_total{SyntheticEntry{1, 5, "a"},
                               SyntheticEntry{2, 6, "b"}};
  if (!rejects_batch(exact_limits, three_entries) ||
      !rejects_batch(exact_limits, long_paths) ||
      !rejects_batch(exact_limits, large_entry) ||
      !rejects_batch(exact_limits, large_total)) {
    return fail("bridge quota excess was accepted");
  }
  return true;
}

[[nodiscard]] bool test_layout_conflicts_and_token_authority() {
  const auto layout_failure = [](const std::span<const SyntheticEntry> entries,
                                 const ohl::media::PayloadLayoutError expected) {
    ParserResultSession session{idle_protocol(), next_worker_epoch()};
    if (!begin_enumeration(session) || !accept_batch(session, entries)) {
      return false;
    }
    constexpr auto payload = complete_payload();
    const auto result = session.complete_enumeration(
        frame(MessageType::complete, payload, 1));
    return result.error == ParserResultSessionError::result_validation_failure &&
           result.layout_error == expected && session.terminal() &&
           !session.catalog().has_value() &&
           session.protocol_state() == SessionState::enumerating;
  };
  const std::array conflicting{SyntheticEntry{1, 1, "Folder/Item.bin"},
                               SyntheticEntry{2, 1, "folder/item.BIN"}};
  const std::array invalid_path{SyntheticEntry{1, 1, "../escape.bin"}};
  if (!layout_failure(conflicting,
                      ohl::media::PayloadLayoutError::path_conflict) ||
      !layout_failure(invalid_path,
                      ohl::media::PayloadLayoutError::invalid_path)) {
    return fail("normalized catalog conflict was promoted");
  }

  const std::array entry{SyntheticEntry{7, 1, "valid/file.bin"}};
  ParserResultSession unknown{idle_protocol(), next_worker_epoch()};
  if (!build_catalog(unknown, entry)) {
    return fail("unknown-token fixture catalog failed");
  }
  const auto generation = unknown.catalog()->generation;
  const auto unknown_payload = stream_entry_payload(8);
  const auto unknown_result = unknown.begin_stream_entry(
      frame(MessageType::stream_entry, unknown_payload, 2), generation);
  if (unknown_result.error != ParserResultSessionError::unknown_source_token ||
      unknown.protocol_state() != SessionState::idle || !unknown.terminal() ||
      unknown.catalog().has_value()) {
    return fail("unknown token received catalog authority");
  }

  const auto stale_epoch = next_worker_epoch();
  ParserResultSession stale{idle_protocol(), stale_epoch};
  if (!build_catalog(stale, entry)) {
    return fail("stale-generation fixture catalog failed");
  }
  const auto old_generation = stale.catalog()->generation;
  if (!begin_enumeration(stale, 2) || stale.catalog().has_value() ||
      !accept_batch(stale, entry, 2) || !complete_enumeration(stale, 2)) {
    return fail("replacement catalog did not retire the old generation");
  }
  const auto new_generation = stale.catalog()->generation;
  const auto replay_payload = stream_entry_payload(7);
  const auto stale_result = stale.begin_stream_entry(
      frame(MessageType::stream_entry, replay_payload, 3), old_generation);
  if (new_generation != ParserResultCatalogGeneration{stale_epoch, 2} ||
      new_generation == old_generation ||
      stale_result.error != ParserResultSessionError::unknown_source_token ||
      stale.protocol_state() != SessionState::idle || !stale.terminal() ||
      stale.catalog().has_value()) {
    return fail("stale generation replay was accepted");
  }
  return true;
}

[[nodiscard]] bool test_stream_failures() {
  const auto prepared = [](const std::uint64_t size) {
    return std::array{SyntheticEntry{1, size, "stream/file.bin"}};
  };

  {
    ParserResultSession session{idle_protocol(), next_worker_epoch()};
    const auto entries = prepared(1);
    if (!build_catalog(session, entries) ||
        !begin_stream(session, 1, session.catalog()->generation, 2)) {
      return fail("malformed-chunk fixture setup failed");
    }
    const std::array chunk{std::byte{0x11}};
    auto malformed = frame(MessageType::data_chunk, chunk, 2);
    malformed.header.flags = 1;
    RecordingSink sink;
    const auto result = session.accept_data_chunk(malformed, sink);
    if (result.error != ParserResultSessionError::protocol_failure ||
        result.protocol_error != ProtocolError::reserved_flags ||
        session.protocol_state() != SessionState::streaming ||
        sink.calls() != 0) {
      return fail("malformed data chunk reached destination or state");
    }
  }
  {
    ParserResultSession session{idle_protocol(), next_worker_epoch()};
    const auto entries = prepared(1);
    if (!build_catalog(session, entries) ||
        !begin_stream(session, 1, session.catalog()->generation, 2)) {
      return fail("empty-chunk fixture setup failed");
    }
    RecordingSink sink;
    const auto result = session.accept_data_chunk(
        frame(MessageType::data_chunk, {}, 2), sink);
    if (result.error != ParserResultSessionError::protocol_failure ||
        result.protocol_error != ProtocolError::noncanonical_value ||
        sink.calls() != 0) {
      return fail("empty data chunk was accepted");
    }
  }
  {
    ParserResultSession session{idle_protocol(), next_worker_epoch()};
    const auto entries = prepared(1);
    if (!build_catalog(session, entries) ||
        !begin_stream(session, 1, session.catalog()->generation, 2)) {
      return fail("oversized-chunk fixture setup failed");
    }
    const std::array oversized{std::byte{0x11}, std::byte{0x22}};
    RecordingSink sink;
    const auto result = session.accept_data_chunk(
        frame(MessageType::data_chunk, oversized, 2), sink);
    if (result.error != ParserResultSessionError::protocol_failure ||
        result.protocol_error != ProtocolError::noncanonical_value ||
        sink.calls() != 0) {
      return fail("chunk exceeding remaining bytes was accepted");
    }
  }
  {
    ParserResultSession session{idle_protocol(), next_worker_epoch()};
    const auto entries = prepared(ohl::parser::kMaximumDataChunkBytes + 1U);
    if (!build_catalog(session, entries) ||
        !begin_stream(session, 1, session.catalog()->generation, 2)) {
      return fail("schema-oversize chunk fixture setup failed");
    }
    std::vector<std::byte> oversized(
        ohl::parser::kMaximumDataChunkBytes + 1U, std::byte{0x44});
    RecordingSink sink;
    const auto result = session.accept_data_chunk(
        frame(MessageType::data_chunk, oversized, 2), sink);
    if (result.error != ParserResultSessionError::protocol_failure ||
        result.protocol_error != ProtocolError::noncanonical_value ||
        sink.calls() != 0) {
      return fail("chunk exceeding schema maximum was accepted");
    }
  }
  {
    ParserResultSession session{idle_protocol(), next_worker_epoch()};
    const auto entries = prepared(3);
    if (!build_catalog(session, entries) ||
        !begin_stream(session, 1, session.catalog()->generation, 2)) {
      return fail("sink-rejection fixture setup failed");
    }
    const std::array chunk{std::byte{0x11}, std::byte{0x22}};
    RecordingSink sink{false, &session};
    const auto result = session.accept_data_chunk(
        frame(MessageType::data_chunk, chunk, 2), sink);
    if (result.error != ParserResultSessionError::downstream_failure ||
        sink.calls() != 1 || sink.observed_remainder() != 3 ||
        session.remaining_stream_bytes() != 0 || !session.terminal() ||
        session.catalog().has_value()) {
      return fail("sink rejection changed remainder before write result");
    }
  }
  {
    ParserResultSession session{idle_protocol(), next_worker_epoch()};
    const auto entries = prepared(2);
    if (!build_catalog(session, entries) ||
        !begin_stream(session, 1, session.catalog()->generation, 2)) {
      return fail("incomplete-stream fixture setup failed");
    }
    constexpr auto payload = complete_payload();
    const auto result = session.complete_stream(
        frame(MessageType::complete, payload, 2));
    if (result.error != ParserResultSessionError::incomplete_stream ||
        session.protocol_state() != SessionState::streaming ||
        !session.terminal() || session.catalog().has_value()) {
      return fail("incomplete stream completion was observed by protocol");
    }
  }
  return true;
}

[[nodiscard]] bool test_decode_allocation_and_aggregate_failures() {
  const PayloadImportLimits limits{1, 32, 8, 8};
  const std::array entry{SyntheticEntry{1, 1, "file.bin"}};
  const auto payload = entry_batch_payload(entry);

  {
    ParserResultSession session{idle_protocol(), next_worker_epoch(), limits};
    allocation_injection::fail_next(allocation_injection::Mode::bad_alloc);
    const auto result = session.begin_enumeration(
        frame(MessageType::enumerate, {}, 1));
    if (result.error != ParserResultSessionError::allocation_failure ||
        session.protocol_state() != SessionState::idle ||
        session.catalog().has_value()) {
      return fail("begin allocation failure changed protocol state");
    }
  }
  {
    ParserResultSession session{idle_protocol(), next_worker_epoch(), limits};
    allocation_injection::fail_next(
        allocation_injection::Mode::other_exception);
    const auto result = session.begin_enumeration(
        frame(MessageType::enumerate, {}, 1));
    if (result.error != ParserResultSessionError::internal_failure ||
        session.protocol_state() != SessionState::idle ||
        session.catalog().has_value()) {
      return fail("unexpected begin exception changed protocol state");
    }
  }
  {
    ParserResultSession session{idle_protocol(), next_worker_epoch(), limits};
    if (!begin_enumeration(session)) {
      return fail("batch allocation fixture setup failed");
    }
    allocation_injection::fail_next(allocation_injection::Mode::bad_alloc);
    const auto result = session.accept_entry_batch(
        frame(MessageType::entry_batch, payload, 1));
    if (result.error != ParserResultSessionError::allocation_failure ||
        session.protocol_state() != SessionState::enumerating ||
        session.catalog().has_value()) {
      return fail("batch allocation failure changed protocol state");
    }
  }
  {
    ParserResultSession session{idle_protocol(), next_worker_epoch(), limits};
    if (!begin_enumeration(session) ||
        !session.accept_entry_batch(
                    frame(MessageType::entry_batch, payload, 1))
             .valid()) {
      return fail("completion allocation fixture setup failed");
    }
    allocation_injection::fail_next(allocation_injection::Mode::bad_alloc);
    constexpr auto complete = complete_payload();
    const auto result = session.complete_enumeration(
        frame(MessageType::complete, complete, 1));
    if (result.error != ParserResultSessionError::allocation_failure ||
        session.protocol_state() != SessionState::enumerating ||
        session.catalog().has_value()) {
      return fail("completion allocation failure promoted a catalog");
    }
  }
  {
    ParserResultSession session{idle_protocol(), next_worker_epoch(), limits};
    if (!begin_enumeration(session)) {
      return fail("decode failure fixture setup failed");
    }
    const std::array<std::byte, 2> empty_batch{};
    const auto result = session.accept_entry_batch(
        frame(MessageType::entry_batch, empty_batch, 1));
    if (result.error != ParserResultSessionError::protocol_failure ||
        result.protocol_error != ProtocolError::noncanonical_value ||
        session.protocol_state() != SessionState::enumerating ||
        session.catalog().has_value()) {
      return fail("entry decode failure changed state or promoted catalog");
    }
  }
  return true;
}

[[nodiscard]] bool test_cancellation_races() {
  const std::array entry{SyntheticEntry{1, 2, "cancel/file.bin"}};
  constexpr auto complete = complete_payload();

  {
    ParserResultSession session{idle_protocol(), next_worker_epoch()};
    if (!begin_enumeration(session)) {
      return fail("enumeration cancellation setup failed");
    }
    if (!session.accept_cancel(frame(MessageType::cancel, {}, 1)).valid() ||
        session.catalog().has_value() || !accept_batch(session, entry) ||
        !session.complete_enumeration(
                    frame(MessageType::complete, complete, 1))
             .valid() ||
        session.catalog().has_value() ||
        session.protocol_state() != SessionState::idle) {
      return fail("cancel-first enumeration race failed closed incorrectly");
    }
  }
  {
    ParserResultSession session{idle_protocol(), next_worker_epoch()};
    if (!build_catalog(session, entry) ||
        !begin_stream(session, 1, session.catalog()->generation, 2) ||
        !session.accept_cancel(frame(MessageType::cancel, {}, 2)).valid()) {
      return fail("stream cancellation setup failed");
    }
    const std::array chunk{std::byte{0x10}, std::byte{0x20}};
    RecordingSink sink;
    if (!session.accept_data_chunk(
                    frame(MessageType::data_chunk, chunk, 2), sink)
             .valid() ||
        !session.complete_stream(frame(MessageType::complete, complete, 2))
             .valid() ||
        session.catalog().has_value() ||
        session.protocol_state() != SessionState::idle) {
      return fail("cancel-first stream race failed");
    }
  }
  {
    ParserResultSession session{idle_protocol(), next_worker_epoch()};
    if (!build_catalog(session, entry) || !session.catalog().has_value() ||
        !session.accept_cancel(frame(MessageType::cancel, {}, 1)).valid() ||
        session.catalog().has_value() ||
        session.protocol_state() != SessionState::idle) {
      return fail("complete-first stale cancel did not retire catalog");
    }
  }
  {
    ParserResultSession session{idle_protocol(), next_worker_epoch()};
    if (!begin_enumeration(session) ||
        !session.accept_cancel(frame(MessageType::cancel, {}, 1)).valid() ||
        !session.accept_cancel_ack(frame(MessageType::cancel_ack, {}, 1))
             .valid() ||
        session.protocol_state() != SessionState::cancelled ||
        !session.accept_shutdown(frame(MessageType::shutdown, {}, 0)).valid() ||
        session.protocol_state() != SessionState::closed) {
      return fail("cancel acknowledgement/shutdown lifecycle failed");
    }
  }
  return true;
}

[[nodiscard]] std::array<std::byte, ohl::parser::kReadRequestPayloadBytes>
read_request_payload() {
  std::array<std::byte, ohl::parser::kReadRequestPayloadBytes> payload{};
  const auto encoded = ohl::parser::encode_read_request_payload(
      {1, 0, 1}, {1, 1}, 1, payload);
  if (!encoded.valid()) {
    std::abort();
  }
  return payload;
}

[[nodiscard]] std::array<std::byte, ohl::parser::kReadReplyPrefixBytes>
read_failure_payload(const ProtocolStatus status) {
  std::array<std::byte, ohl::parser::kReadReplyPrefixBytes> payload{};
  const auto encoded = ohl::parser::encode_read_reply_payload(
      {1, status, {}}, 1, 1, payload);
  if (!encoded.valid()) {
    std::abort();
  }
  return payload;
}

[[nodiscard]] std::array<std::byte, ohl::parser::kReadReplyPrefixBytes + 1>
read_success_payload() {
  std::array<std::byte, ohl::parser::kReadReplyPrefixBytes + 1> payload{};
  const std::array bytes{std::byte{0x5a}};
  const auto encoded = ohl::parser::encode_read_reply_payload(
      {1, ProtocolStatus::ok, bytes}, 1, 1, payload);
  if (!encoded.valid()) {
    std::abort();
  }
  return payload;
}

[[nodiscard]] bool test_cancellation_read_races() {
  const std::array entry{SyntheticEntry{1, 0, "cancel/read.bin"}};
  const auto activate_request = [&](ParserResultSession& session,
                                    const bool streaming) {
    if (!streaming) {
      return begin_enumeration(session);
    }
    return build_catalog(session, entry) &&
           begin_stream(session, 1, session.catalog()->generation, 2);
  };

  for (const bool streaming : {false, true}) {
    const std::uint64_t request_id = streaming ? 2 : 1;

    {
      ParserResultSession session{idle_protocol(), next_worker_epoch()};
      if (!activate_request(session, streaming) ||
          !session
               .accept_cancel(frame(MessageType::cancel, {}, request_id))
               .valid()) {
        return fail("post-cancel read fixture setup failed");
      }
      const auto request = read_request_payload();
      const auto ignored = session.accept_read_request(
          frame(MessageType::read_request, request, request_id), {1, 1}, 1);
      if (!ignored.valid() || ignored.serviceable() ||
          ignored.disposition !=
              ParserReadRequestDisposition::ignored_after_cancel ||
          ignored.message.read_sequence != 0 || ignored.message.offset != 0 ||
          ignored.message.length != 0 ||
          session.protocol_state() != SessionState::cancelling ||
          !session
               .accept_cancel_ack(
                   frame(MessageType::cancel_ack, {}, request_id))
               .valid() ||
          session.protocol_state() != SessionState::cancelled) {
        return fail("post-cancel crossed read was not ignored through ack");
      }
    }

    {
      ParserResultSession session{idle_protocol(), next_worker_epoch()};
      if (!activate_request(session, streaming)) {
        return fail("pre-cancel read fixture setup failed");
      }
      const auto request = read_request_payload();
      const auto serviceable = session.accept_read_request(
          frame(MessageType::read_request, request, request_id), {1, 1}, 1);
      if (!serviceable.serviceable() ||
          serviceable.disposition !=
              ParserReadRequestDisposition::serviceable ||
          serviceable.message.read_sequence != 1 ||
          serviceable.message.offset != 0 || serviceable.message.length != 1 ||
          !session
               .accept_cancel(frame(MessageType::cancel, {}, request_id))
               .valid()) {
        return fail("pre-cancel read was not retained during cancellation");
      }
      const auto reply = read_success_payload();
      if (!session
               .accept_read_reply(
                   frame(MessageType::read_reply, reply, request_id), 1, 1)
               .valid() ||
          !session
               .accept_cancel_ack(
                   frame(MessageType::cancel_ack, {}, request_id))
               .valid() ||
          session.protocol_state() != SessionState::cancelled) {
        return fail("legitimate pre-cancel crossing read reply was rejected");
      }
    }

    {
      ParserResultSession session{idle_protocol(), next_worker_epoch()};
      if (!activate_request(session, streaming) ||
          !session
               .accept_cancel(frame(MessageType::cancel, {}, request_id))
               .valid()) {
        return fail("unserviceable post-cancel read fixture setup failed");
      }
      const auto request = read_request_payload();
      const auto ignored = session.accept_read_request(
          frame(MessageType::read_request, request, request_id), {1, 1}, 1);
      const auto reply = read_success_payload();
      const auto result = session.accept_read_reply(
          frame(MessageType::read_reply, reply, request_id), 1, 1);
      if (!ignored.valid() || ignored.serviceable() ||
          ignored.disposition !=
              ParserReadRequestDisposition::ignored_after_cancel ||
          result.error != ParserResultSessionError::protocol_failure ||
          result.protocol_error != ProtocolError::no_read_in_flight ||
          !session.terminal()) {
        return fail("post-cancel read incorrectly authorized a reply");
      }
    }
  }
  return true;
}

[[nodiscard]] bool test_reads_and_terminal_lifecycle() {
  const auto read_failure = [](const ProtocolStatus status,
                               const ParserResultSessionError expected) {
    ParserResultSession session{idle_protocol(), next_worker_epoch()};
    if (!begin_enumeration(session)) {
      return false;
    }
    const auto request = read_request_payload();
    const auto decoded = session.accept_read_request(
        frame(MessageType::read_request, request, 1), {1, 1}, 1);
    if (!decoded.serviceable() ||
        decoded.disposition != ParserReadRequestDisposition::serviceable ||
        decoded.message.length != 1) {
      return false;
    }
    const auto reply = read_failure_payload(status);
    const auto result = session.accept_read_reply(
        frame(MessageType::read_reply, reply, 1), 1, 1);
    return result.error == expected && session.terminal() &&
           !session.catalog().has_value();
  };
  if (!read_failure(ProtocolStatus::source_changed,
                    ParserResultSessionError::source_invalidated) ||
      !read_failure(ProtocolStatus::source_read_failed,
                    ParserResultSessionError::source_read_failure)) {
    return fail("non-ok read reply did not retire the session");
  }

  const std::array entry{SyntheticEntry{1, 0, "life/file.bin"}};
  {
    ParserResultSession session{idle_protocol(), next_worker_epoch()};
    if (!build_catalog(session, entry)) {
      return fail("source invalidation fixture setup failed");
    }
    session.invalidate_source();
    if (session.result().error !=
            ParserResultSessionError::source_invalidated ||
        !session.terminal() || session.catalog().has_value()) {
      return fail("source invalidation did not retire catalog");
    }
  }
  {
    ParserResultSession session{idle_protocol(), next_worker_epoch()};
    if (!build_catalog(session, entry) ||
        !session.accept_shutdown(frame(MessageType::shutdown, {}, 0)).valid() ||
        session.protocol_state() != SessionState::closed ||
        session.catalog().has_value()) {
      return fail("shutdown did not retire catalog and close protocol");
    }
  }
  {
    const auto old_epoch = next_worker_epoch();
    ParserResultSession old_worker{idle_protocol(), old_epoch};
    if (!build_catalog(old_worker, entry)) {
      return fail("worker restart fixture setup failed");
    }
    const auto old_generation = old_worker.catalog()->generation;
    old_worker.worker_failed();
    if (old_worker.result().error != ParserResultSessionError::worker_failure ||
        !old_worker.terminal() || old_worker.catalog().has_value()) {
      return fail("worker failure did not retire old catalog");
    }
    const auto restarted_epoch = next_worker_epoch();
    ParserResultSession restarted{idle_protocol(), restarted_epoch};
    if (!build_catalog(restarted, entry) ||
        old_generation != ParserResultCatalogGeneration{old_epoch, 1} ||
        restarted.catalog()->generation !=
            ParserResultCatalogGeneration{restarted_epoch, 1} ||
        restarted.catalog()->generation == old_generation ||
        restarted.catalog()->entries[0].source_token != 1) {
      return fail("worker restart did not establish independent catalog");
    }
    const auto replay_payload = stream_entry_payload(1);
    const auto replay = restarted.begin_stream_entry(
        frame(MessageType::stream_entry, replay_payload, 2), old_generation);
    if (replay.error != ParserResultSessionError::unknown_source_token ||
        restarted.protocol_state() != SessionState::idle ||
        !restarted.terminal() || restarted.catalog().has_value()) {
      return fail("old generation replay crossed a worker restart");
    }
  }
  return true;
}

[[nodiscard]] bool test_invalid_configuration() {
  const PayloadImportLimits invalid{
      .maximum_entries = 1,
      .maximum_path_bytes = 1,
      .maximum_entry_bytes = 2,
      .maximum_total_bytes = 1,
  };
  ParserResultSession session{idle_protocol(), next_worker_epoch(), invalid};
  ProtocolStateValidator awaiting_hello{kSession};
  ParserResultSession invalid_state_protocol{std::move(awaiting_hello),
                                              next_worker_epoch()};
  ParserResultSession zero_epoch{idle_protocol(), 0};
  if (!session.terminal() ||
      session.result().error !=
          ParserResultSessionError::invalid_configuration ||
      session.protocol_state() != SessionState::idle ||
      session.catalog().has_value() || !invalid_state_protocol.terminal() ||
      invalid_state_protocol.result().error !=
          ParserResultSessionError::invalid_configuration ||
      invalid_state_protocol.protocol_state() != SessionState::awaiting_hello ||
      !zero_epoch.terminal() ||
      zero_epoch.result().error !=
          ParserResultSessionError::invalid_configuration ||
      zero_epoch.protocol_state() != SessionState::idle) {
    return fail("invalid bridge configuration was accepted");
  }

  ParserResultSession wrong_lifecycle{idle_protocol(), next_worker_epoch()};
  const std::array entry{SyntheticEntry{1, 1, "state/file.bin"}};
  const auto payload = entry_batch_payload(entry);
  const auto result = wrong_lifecycle.accept_entry_batch(
      frame(MessageType::entry_batch, payload, 1));
  return result.error == ParserResultSessionError::invalid_state &&
                 wrong_lifecycle.terminal() &&
                 wrong_lifecycle.protocol_state() == SessionState::idle &&
                 !wrong_lifecycle.catalog().has_value()
             ? true
             : fail("out-of-order bridge lifecycle was accepted");
}

}  // namespace

int main() {
  return test_catalog_promotion_and_streams() &&
                 test_empty_and_quota_promotions() &&
                 test_layout_conflicts_and_token_authority() &&
                 test_stream_failures() &&
                 test_decode_allocation_and_aggregate_failures() &&
                 test_cancellation_races() &&
                 test_cancellation_read_races() &&
                 test_reads_and_terminal_lifecycle() &&
                 test_invalid_configuration()
             ? 0
             : 1;
}
