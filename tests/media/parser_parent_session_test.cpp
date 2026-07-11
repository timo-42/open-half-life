#include "ohl/media/parser_parent_session.hpp"

#include "synthetic_media_test_support.hpp"

#include <algorithm>
#include <array>
#include <chrono>
#include <condition_variable>
#include <cstddef>
#include <cstdint>
#include <deque>
#include <iostream>
#include <limits>
#include <memory>
#include <mutex>
#include <span>
#include <stdexcept>
#include <string_view>
#include <thread>
#include <utility>
#include <vector>

namespace {

using Clock = std::chrono::steady_clock;
using ohl::media::ParserFrameChannel;
using ohl::media::ParserFrameChannelOperations;
using ohl::media::ParserParentSession;
using ohl::media::ParserParentSessionDisposition;
using ohl::media::ParserParentSessionError;
using ohl::media::ParserParentSessionResult;
using ohl::media::ParserParentSessionState;
using ohl::media::ParserResultCatalogGeneration;
using ohl::media::ParserSourceReadBrokerError;
using ohl::media::ParserSourceReadLimits;
using ohl::media::PayloadByteSink;
using ohl::media::PayloadImportLimits;
using ohl::parser::MessageType;
using ohl::parser::ProtocolBudgets;
using ohl::parser::ProtocolError;
using ohl::platform::IsolatedWorkerCancellationToken;
using ohl::platform::IsolatedWorkerError;
using ohl::platform::IsolatedWorkerIoResult;

constexpr std::uint64_t kSession = 0x1234'5678'9abc'def0ULL;
constexpr std::uint64_t kEpoch = 0x4000'0000'0000'002aULL;
constexpr std::uint64_t kMarkerOffset =
    100U * ohl::media::test::kSyntheticSectorSize;
constexpr std::size_t kExact = std::numeric_limits<std::size_t>::max();

template <typename T>
concept HasFrame = requires(T value) { value.frame; };
template <typename T>
concept HasPayload = requires(T value) { value.payload; };
static_assert(!HasFrame<ParserParentSessionResult>);
static_assert(!HasPayload<ParserParentSessionResult>);

[[nodiscard]] bool fail(const std::string_view message) {
  std::cerr << message << '\n';
  return false;
}

void store_u16(const std::span<std::byte> out, const std::size_t offset,
               const std::uint16_t value) {
  out[offset] = static_cast<std::byte>(value & 0xffU);
  out[offset + 1] = static_cast<std::byte>((value >> 8U) & 0xffU);
}
void store_u32(const std::span<std::byte> out, const std::size_t offset,
               const std::uint32_t value) {
  for (std::size_t i = 0; i < 4; ++i) {
    out[offset + i] = static_cast<std::byte>((value >> (i * 8U)) & 0xffU);
  }
}
void store_u64(const std::span<std::byte> out, const std::size_t offset,
               const std::uint64_t value) {
  for (std::size_t i = 0; i < 8; ++i) {
    out[offset + i] = static_cast<std::byte>((value >> (i * 8U)) & 0xffU);
  }
}
[[nodiscard]] std::uint16_t load_u16(const std::span<const std::byte> in,
                                     const std::size_t offset) {
  return static_cast<std::uint16_t>(
      std::to_integer<std::uint16_t>(in[offset]) |
      (std::to_integer<std::uint16_t>(in[offset + 1]) << 8U));
}
[[nodiscard]] std::uint32_t load_u32(const std::span<const std::byte> in,
                                     const std::size_t offset) {
  std::uint32_t value = 0;
  for (std::size_t i = 0; i < 4; ++i) {
    value |= std::to_integer<std::uint32_t>(in[offset + i]) << (i * 8U);
  }
  return value;
}
[[nodiscard]] std::uint64_t load_u64(const std::span<const std::byte> in,
                                     const std::size_t offset) {
  std::uint64_t value = 0;
  for (std::size_t i = 0; i < 8; ++i) {
    value |= std::to_integer<std::uint64_t>(in[offset + i]) << (i * 8U);
  }
  return value;
}

[[nodiscard]] std::array<std::byte, 32> wire_header(
    const MessageType type, const std::uint32_t payload_size,
    const std::uint64_t request_id, const std::uint64_t session = kSession) {
  std::array<std::byte, 32> out{};
  out[0] = std::byte{'O'};
  out[1] = std::byte{'H'};
  out[2] = std::byte{'L'};
  out[3] = std::byte{'P'};
  store_u16(out, 4, 1);
  store_u16(out, 8, static_cast<std::uint16_t>(type));
  store_u32(out, 12, payload_size);
  store_u64(out, 16, session);
  store_u64(out, 24, request_id);
  return out;
}

struct ReadPlan {
  std::vector<std::byte> bytes;
  std::size_t transferred{kExact};
  IsolatedWorkerError error{IsolatedWorkerError::none};
};
struct WriteOutcome {
  bool configured{false};
  std::size_t transferred{kExact};
  IsolatedWorkerError error{IsolatedWorkerError::none};
};
struct Observation {
  std::size_t size{0};
  std::array<std::byte, 64> prefix{};
  std::size_t prefix_size{0};
};
struct VisibilityObservation {
  bool observed{false};
  ParserParentSessionState state{ParserParentSessionState::terminal};
  ParserParentSessionError result_error{ParserParentSessionError::internal_failure};
  bool catalog_visible{false};
  ParserResultCatalogGeneration generation{};
  std::size_t catalog_entries{0};
};

class DuplexIo final {
 public:
  [[nodiscard]] ParserFrameChannelOperations operations() noexcept {
    return {read_exact, write_all, abort_io, this};
  }
  void push_read(const std::span<const std::byte> bytes,
                 const std::size_t transferred = kExact,
                 const IsolatedWorkerError error = IsolatedWorkerError::none) {
    std::scoped_lock lock{mutex_};
    reads_.push_back({std::vector<std::byte>{bytes.begin(), bytes.end()},
                      transferred, error});
    condition_.notify_all();
  }
  void push_frame(const MessageType type, const std::uint64_t request,
                  const std::span<const std::byte> payload = {}) {
    const auto header = wire_header(type, static_cast<std::uint32_t>(payload.size()),
                                    request);
    push_read(header);
    if (!payload.empty()) {
      push_read(payload);
    }
  }
  void set_write_failure(const std::size_t call, const std::size_t transferred,
                         const IsolatedWorkerError error) {
    std::scoped_lock lock{mutex_};
    write_outcomes_[call] = {true, transferred, error};
  }
  void block_reads_when_empty() {
    std::scoped_lock lock{mutex_};
    block_empty_reads_ = true;
  }
  void block_write(const std::size_t call) {
    std::scoped_lock lock{mutex_};
    blocked_write_call_ = call;
  }
  void reenter_from_callbacks(ParserParentSession& session,
                              const std::size_t write_call,
                              const bool abort_callback) {
    std::scoped_lock lock{mutex_};
    reenter_session_ = &session;
    reenter_write_call_ = write_call;
    reenter_abort_ = abort_callback;
  }
  [[nodiscard]] bool wait_for_reads(const std::size_t count) {
    std::unique_lock lock{mutex_};
    return condition_.wait_for(lock, std::chrono::seconds{5},
                               [&] { return read_calls_ >= count; });
  }
  [[nodiscard]] bool wait_for_writes(const std::size_t count) {
    std::unique_lock lock{mutex_};
    return condition_.wait_for(lock, std::chrono::seconds{5},
                               [&] { return write_calls_ >= count; });
  }
  [[nodiscard]] bool wait_for_visibility() {
    std::unique_lock lock{mutex_};
    return condition_.wait_for(lock, std::chrono::seconds{5},
                               [&] { return visibility_.observed; });
  }
  [[nodiscard]] std::size_t read_calls() const {
    std::scoped_lock lock{mutex_};
    return read_calls_;
  }
  [[nodiscard]] std::size_t write_calls() const {
    std::scoped_lock lock{mutex_};
    return write_calls_;
  }
  [[nodiscard]] std::size_t abort_calls() const {
    std::scoped_lock lock{mutex_};
    return abort_calls_;
  }
  [[nodiscard]] bool callbacks_reentered() const {
    std::scoped_lock lock{mutex_};
    return write_reentered_ && (!reenter_abort_ || abort_reentered_);
  }
  [[nodiscard]] Observation write(const std::size_t index) const {
    std::scoped_lock lock{mutex_};
    return writes_[index];
  }
  [[nodiscard]] VisibilityObservation visibility() const {
    std::scoped_lock lock{mutex_};
    return visibility_;
  }

 private:
  static IsolatedWorkerIoResult read_exact(
      void* opaque, const std::span<std::byte> destination, Clock::time_point,
      IsolatedWorkerCancellationToken) noexcept {
    auto& io = *static_cast<DuplexIo*>(opaque);
    std::unique_lock lock{io.mutex_};
    ++io.read_calls_;
    io.condition_.notify_all();
    if (io.block_empty_reads_) {
      io.condition_.wait(lock,
                         [&] { return io.aborted_ || !io.reads_.empty(); });
    }
    if (io.aborted_) {
      return {0, IsolatedWorkerError::cancelled};
    }
    if (io.reads_.empty()) {
      return {0, IsolatedWorkerError::io_failure};
    }
    auto plan = std::move(io.reads_.front());
    io.reads_.pop_front();
    lock.unlock();
    const auto transferred =
        plan.transferred == kExact ? destination.size() : plan.transferred;
    const auto copied =
        std::min({destination.size(), plan.bytes.size(), transferred});
    std::copy_n(plan.bytes.begin(), copied, destination.begin());
    return {transferred, plan.error};
  }
  static IsolatedWorkerIoResult write_all(
      void* opaque, const std::span<const std::byte> source, Clock::time_point,
      IsolatedWorkerCancellationToken) noexcept {
    auto& io = *static_cast<DuplexIo*>(opaque);
    std::unique_lock lock{io.mutex_};
    const auto call = io.write_calls_++;
    auto& observation = io.writes_[call];
    observation.size = source.size();
    observation.prefix_size = std::min(source.size(), observation.prefix.size());
    std::copy_n(source.begin(), observation.prefix_size,
                observation.prefix.begin());
    const auto outcome = io.write_outcomes_[call];
    const bool reenter = io.reenter_session_ != nullptr &&
                         io.reenter_write_call_ == call;
    auto* const session = io.reenter_session_;
    io.condition_.notify_all();
    lock.unlock();
    if (reenter) {
      VisibilityObservation visible;
      visible.observed = true;
      visible.state = session->state();
      visible.result_error = session->result().error;
      const auto catalog = session->catalog();
      visible.catalog_visible = catalog.has_value();
      if (catalog) {
        visible.generation = catalog->generation;
        visible.catalog_entries = catalog->entries.size();
      }
      lock.lock();
      io.visibility_ = visible;
      lock.unlock();
    }
    lock.lock();
    io.write_reentered_ = io.write_reentered_ || reenter;
    io.condition_.notify_all();
    if (io.blocked_write_call_ == call) {
      io.condition_.wait(lock, [&] { return io.aborted_; });
    }
    if (io.aborted_) return {0, IsolatedWorkerError::cancelled};
    return {outcome.configured && outcome.transferred != kExact
                ? outcome.transferred
                : source.size(),
            outcome.configured ? outcome.error : IsolatedWorkerError::none};
  }
  static void abort_io(void* opaque) noexcept {
    auto& io = *static_cast<DuplexIo*>(opaque);
    ParserParentSession* session = nullptr;
    bool reenter = false;
    {
      std::scoped_lock lock{io.mutex_};
      ++io.abort_calls_;
      io.aborted_ = true;
      session = io.reenter_session_;
      reenter = io.reenter_abort_ && session != nullptr;
    }
    io.condition_.notify_all();
    if (reenter) {
      static_cast<void>(session->state());
      static_cast<void>(session->result());
      std::scoped_lock lock{io.mutex_};
      io.abort_reentered_ = true;
    }
  }

  mutable std::mutex mutex_;
  std::condition_variable condition_;
  std::deque<ReadPlan> reads_;
  std::array<WriteOutcome, 128> write_outcomes_{};
  std::array<Observation, 128> writes_{};
  std::size_t read_calls_{0};
  std::size_t write_calls_{0};
  std::size_t abort_calls_{0};
  bool block_empty_reads_{false};
  bool aborted_{false};
  std::size_t blocked_write_call_{std::numeric_limits<std::size_t>::max()};
  ParserParentSession* reenter_session_{nullptr};
  std::size_t reenter_write_call_{std::numeric_limits<std::size_t>::max()};
  bool reenter_abort_{false};
  bool write_reentered_{false};
  bool abort_reentered_{false};
  VisibilityObservation visibility_{};
};

[[nodiscard]] ParserSourceReadLimits small_read_limits(
    const std::uint64_t requests = 16,
    const std::uint64_t reply_bytes = 160) {
  return {.maximum_read_bytes = 4,
          .maximum_requests = requests,
          .maximum_reply_payload_bytes = reply_bytes};
}

struct Buffers {
  explicit Buffers(const ParserSourceReadLimits limits)
      : receive(ohl::parser::kMaximumFramePayloadBytes, std::byte{0xa5}),
        scratch(limits.maximum_read_bytes, std::byte{0xb6}),
        reply(ohl::parser::kReadReplyPrefixBytes + limits.maximum_read_bytes,
              std::byte{0xc7}) {}
  std::vector<std::byte> receive;
  std::vector<std::byte> scratch;
  std::vector<std::byte> reply;
};

class Harness final {
 public:
  Harness(const ParserSourceReadLimits limits = small_read_limits(),
          const ProtocolBudgets budgets = {},
          const PayloadImportLimits imports = {},
          const std::size_t sectors =
              ohl::media::test::kSyntheticMinimumSectorCount)
      : media{sectors}, channel{kSession, io.operations()}, limits_{limits} {
    io.push_frame(MessageType::ready, 0);
    std::vector<std::byte> storage(ohl::parser::kMaximumFramePayloadBytes);
    auto handshake = ohl::media::perform_parser_parent_handshake(
        channel, media.media(), limits, budgets, storage,
        Clock::time_point::max());
    if (!handshake.valid()) {
      throw std::runtime_error{"handshake fixture failed"};
    }
    auto created = ohl::media::create_parser_parent_session(
        std::move(*handshake.proof), channel, media.media(), kEpoch, imports);
    if (!created.valid()) {
      throw std::runtime_error{"parent session fixture failed"};
    }
    session = std::move(created.session);
  }

  [[nodiscard]] Buffers buffers() const { return Buffers{limits_}; }

  ohl::media::test::SyntheticValidatedMedia media;
  DuplexIo io;
  ParserFrameChannel channel;
  std::unique_ptr<ParserParentSession> session;

 private:
  ParserSourceReadLimits limits_;
};

struct Entry {
  std::uint64_t token;
  std::uint64_t size;
  std::string_view path;
};
[[nodiscard]] std::vector<std::byte> entry_batch(
    const std::span<const Entry> entries) {
  std::size_t size = 2;
  for (const auto& entry : entries) size += 18 + entry.path.size();
  std::vector<std::byte> out(size);
  store_u16(out, 0, static_cast<std::uint16_t>(entries.size()));
  std::size_t offset = 2;
  for (const auto& entry : entries) {
    store_u64(out, offset, entry.token);
    store_u64(out, offset + 8, entry.size);
    store_u16(out, offset + 16,
              static_cast<std::uint16_t>(entry.path.size()));
    std::transform(entry.path.begin(), entry.path.end(), out.begin() +
                       static_cast<std::ptrdiff_t>(offset + 18),
                   [](char value) { return static_cast<std::byte>(value); });
    offset += 18 + entry.path.size();
  }
  return out;
}
[[nodiscard]] constexpr std::array<std::byte, 4> complete_payload() {
  return {std::byte{0}, std::byte{0}, std::byte{4}, std::byte{0}};
}
[[nodiscard]] std::array<std::byte, 16> read_request(
    const std::uint32_t sequence, const std::uint64_t offset,
    const std::uint32_t length) {
  std::array<std::byte, 16> out{};
  store_u32(out, 0, sequence);
  store_u64(out, 4, offset);
  store_u32(out, 12, length);
  return out;
}

[[nodiscard]] ParserParentSessionResult receive(Harness& h, Buffers& buffers) {
  return h.session->receive_one(buffers.receive, buffers.scratch, buffers.reply,
                                Clock::time_point::max());
}
[[nodiscard]] bool begin_enum(Harness& h, std::uint64_t& request) {
  const auto result =
      h.session->begin_enumeration(Clock::time_point::max());
  request = result.request_id;
  return result.valid() &&
         result.disposition == ParserParentSessionDisposition::request_sent;
}
[[nodiscard]] bool build_catalog(Harness& h,
                                 const std::span<const Entry> entries,
                                 ParserResultCatalogGeneration& generation) {
  auto buffers = h.buffers();
  std::uint64_t request = 0;
  if (!begin_enum(h, request)) return false;
  if (!entries.empty()) {
    const auto payload = entry_batch(entries);
    h.io.push_frame(MessageType::entry_batch, request, payload);
    if (!receive(h, buffers).valid()) return false;
  }
  constexpr auto complete = complete_payload();
  h.io.push_frame(MessageType::complete, request, complete);
  const auto done = receive(h, buffers);
  const auto catalog = h.session->catalog();
  if (!done.valid() || done.disposition !=
                           ParserParentSessionDisposition::enumeration_complete ||
      !catalog.has_value()) return false;
  generation = catalog->generation;
  return true;
}

class RecordingSink final : public PayloadByteSink {
 public:
  RecordingSink(const std::size_t reject_call = 0,
                const std::size_t partial_on_reject = 0)
      : reject_call_{reject_call}, partial_on_reject_{partial_on_reject} {}
  bool write(const std::span<const std::byte> bytes) noexcept override {
    ++calls_;
    if (calls_ == reject_call_) {
      const auto copied = std::min(partial_on_reject_, bytes.size());
      std::copy_n(bytes.begin(), copied, bytes_.begin() + size_);
      size_ += copied;
      return false;
    }
    std::copy(bytes.begin(), bytes.end(), bytes_.begin() + size_);
    size_ += bytes.size();
    return true;
  }
  std::array<std::byte, 32> bytes_{};
  std::size_t size_{0};
  std::size_t calls_{0};
 private:
  std::size_t reject_call_{0};
  std::size_t partial_on_reject_{0};
};

[[nodiscard]] bool test_construction_contract() {
  ohl::media::test::SyntheticValidatedMedia media;
  DuplexIo io;
  ParserFrameChannel channel{kSession, io.operations()};
  io.push_frame(MessageType::ready, 0);
  std::vector<std::byte> storage(ohl::parser::kMaximumFramePayloadBytes);
  auto handshake = ohl::media::perform_parser_parent_handshake(
      channel, media.media(), small_read_limits(), {}, storage,
      Clock::time_point::max());
  if (!handshake.valid()) return fail("construction handshake failed");

  DuplexIo other_io;
  ParserFrameChannel other_channel{kSession + 1, other_io.operations()};
  auto mismatch = ohl::media::create_parser_parent_session(
      std::move(*handshake.proof), other_channel, media.media(), kEpoch);
  if (mismatch.valid() || !handshake.proof->valid())
    return fail("channel mismatch consumed proof");
  DuplexIo same_id_io;
  ParserFrameChannel same_id_channel{kSession, same_id_io.operations()};
  mismatch = ohl::media::create_parser_parent_session(
      std::move(*handshake.proof), same_id_channel, media.media(), kEpoch);
  if (mismatch.valid() || !handshake.proof->valid())
    return fail("same-ID distinct channel consumed proof");
  ohl::media::test::SyntheticValidatedMedia different_size{
      ohl::media::test::kSyntheticMinimumSectorCount + 1};
  mismatch = ohl::media::create_parser_parent_session(
      std::move(*handshake.proof), channel, different_size.media(), kEpoch);
  if (mismatch.valid() || !handshake.proof->valid())
    return fail("media-size mismatch consumed proof");
  mismatch = ohl::media::create_parser_parent_session(
      std::move(*handshake.proof), channel, media.media(), 0);
  if (mismatch.valid() || !handshake.proof->valid())
    return fail("zero epoch consumed proof");
  PayloadImportLimits invalid{};
  invalid.maximum_entries = 0;
  mismatch = ohl::media::create_parser_parent_session(
      std::move(*handshake.proof), channel, media.media(), kEpoch, invalid);
  if (mismatch.valid() || !handshake.proof->valid())
    return fail("invalid imports consumed proof");
  auto created = ohl::media::create_parser_parent_session(
      std::move(*handshake.proof), channel, media.media(), kEpoch);
  if (!created.valid() || handshake.proof->valid())
    return fail("valid creation did not consume proof once");
  auto reused = ohl::media::create_parser_parent_session(
      std::move(*handshake.proof), channel, media.media(), kEpoch);
  if (reused.valid()) return fail("consumed proof was reused");
  const auto before = io.abort_calls();
  created.session.reset();
  if (io.abort_calls() != before + 1) return fail("active destructor did not abort");

  ohl::media::test::SyntheticValidatedMedia terminal_media;
  DuplexIo terminal_io;
  ParserFrameChannel terminal_channel{kSession, terminal_io.operations()};
  terminal_io.push_frame(MessageType::ready, 0);
  auto terminal_handshake = ohl::media::perform_parser_parent_handshake(
      terminal_channel, terminal_media.media(), small_read_limits(), {}, storage,
      Clock::time_point::max());
  terminal_channel.abort();
  auto terminal_create = ohl::media::create_parser_parent_session(
      std::move(*terminal_handshake.proof), terminal_channel,
      terminal_media.media(), kEpoch);
  return !terminal_create.valid() && terminal_handshake.proof->valid() &&
         terminal_io.abort_calls() == 1;
}

[[nodiscard]] bool test_sticky_send_failures_and_destructor() {
  {
    Harness h;
    h.io.set_write_failure(2, 0, IsolatedWorkerError::timeout);
    const auto failed = h.session->begin_enumeration(Clock::time_point::max());
    const auto writes = h.io.write_calls();
    const auto later = h.session->begin_enumeration(Clock::time_point::max());
    if (failed.error != ParserParentSessionError::channel_failure ||
        later.error != failed.error || !h.session->terminal() ||
        h.io.write_calls() != writes || h.io.abort_calls() != 1)
      return fail("enumeration send failure was not sticky");
  }
  {
    Harness h;
    std::uint64_t request = 0;
    if (!begin_enum(h, request)) return fail("cancel send setup");
    h.io.set_write_failure(3, 0, IsolatedWorkerError::cancelled);
    const auto failed = h.session->request_cancel(Clock::time_point::max());
    if (failed.error != ParserParentSessionError::channel_failure ||
        h.session->result().error != failed.error || h.io.abort_calls() != 1)
      return fail("cancel send failure was not sticky");
  }
  {
    Harness h;
    h.io.set_write_failure(2, 0, IsolatedWorkerError::peer_closed);
    if (h.session->shutdown(Clock::time_point::max()).error !=
            ParserParentSessionError::channel_failure ||
        h.io.abort_calls() != 1)
      return fail("shutdown send failure was not terminal");
  }
  {
    Harness h;
    const auto before = h.io.abort_calls();
    if (h.session->shutdown(Clock::time_point::max()).disposition !=
        ParserParentSessionDisposition::shutdown_sent)
      return fail("closed destructor setup");
    h.session.reset();
    if (h.io.abort_calls() != before)
      return fail("closed destructor aborted channel");
  }
  return true;
}

[[nodiscard]] bool test_enumeration_catalog_and_streams() {
  Harness h;
  auto buffers = h.buffers();
  std::uint64_t empty_request = 0;
  if (!begin_enum(h, empty_request)) return fail("empty enumeration begin");
  const auto enumerate_header = h.io.write(2);
  if (h.io.write_calls() != 3 || enumerate_header.size != 32 ||
      load_u16(enumerate_header.prefix, 8) !=
          static_cast<std::uint16_t>(MessageType::enumerate) ||
      load_u32(enumerate_header.prefix, 12) != 0 ||
      load_u64(enumerate_header.prefix, 24) != empty_request ||
      empty_request != 1)
    return fail("canonical enumeration request framing failed");
  constexpr auto complete = complete_payload();
  h.io.push_frame(MessageType::complete, empty_request, complete);
  if (receive(h, buffers).disposition !=
          ParserParentSessionDisposition::enumeration_complete ||
      !h.session->catalog().has_value() ||
      !h.session->catalog()->entries.empty())
    return fail("empty catalog promotion failed");

  std::uint64_t request = 0;
  if (!begin_enum(h, request) || request != 2)
    return fail("multi enumeration request ID");
  const std::array first{Entry{1, 3, "B/b.bin"}};
  const std::array second{Entry{2, 0, "A/a.bin"}};
  const auto first_payload = entry_batch(first);
  const auto second_payload = entry_batch(second);
  h.io.push_frame(MessageType::entry_batch, request, first_payload);
  h.io.push_frame(MessageType::entry_batch, request, second_payload);
  h.io.push_frame(MessageType::complete, request, complete);
  if (!receive(h, buffers).valid() || !receive(h, buffers).valid() ||
      receive(h, buffers).disposition !=
          ParserParentSessionDisposition::enumeration_complete)
    return fail("multi-batch enumeration failed");
  const auto catalog = h.session->catalog();
  if (!catalog || catalog->entries.size() != 2 || catalog->total_bytes != 3 ||
      catalog->entries[0].relative_path != "A/a.bin" ||
      catalog->entries[1].relative_path != "B/b.bin")
    return fail("catalog normalize/sort failed");
  const auto generation = catalog->generation;

  RecordingSink empty_sink;
  auto started = h.session->begin_stream(generation, 2, empty_sink,
                                         Clock::time_point::max());
  h.io.push_frame(MessageType::complete, started.request_id, complete);
  if (!started.valid() || started.request_id != 3 ||
      receive(h, buffers).disposition !=
                              ParserParentSessionDisposition::stream_complete ||
      empty_sink.calls_ != 0) return fail("empty stream failed");

  RecordingSink sink;
  started = h.session->begin_stream(generation, 1, sink,
                                    Clock::time_point::max());
  const std::array chunk1{std::byte{0x11}};
  const std::array chunk2{std::byte{0x22}, std::byte{0x33}};
  h.io.push_frame(MessageType::data_chunk, started.request_id, chunk1);
  h.io.push_frame(MessageType::data_chunk, started.request_id, chunk2);
  h.io.push_frame(MessageType::complete, started.request_id, complete);
  if (!started.valid() || started.request_id != 4 ||
      !receive(h, buffers).valid() ||
      !receive(h, buffers).valid() ||
      receive(h, buffers).disposition !=
          ParserParentSessionDisposition::stream_complete ||
      sink.size_ != 3 || sink.bytes_[0] != chunk1[0] ||
      sink.bytes_[2] != chunk2[1]) return fail("exact chunk stream failed");

  ParserResultCatalogGeneration replacement{};
  if (!build_catalog(h, {}, replacement) || replacement == generation ||
      replacement.enumeration != 3)
    return fail("catalog replacement failed");
  RecordingSink stale_sink;
  const auto stale = h.session->begin_stream(generation, 1, stale_sink,
                                              Clock::time_point::max());
  return stale.error == ParserParentSessionError::result_failure &&
         h.session->terminal();
}

[[nodiscard]] bool test_sink_rejection_and_stale_token() {
  {
    Harness h;
    const std::array entries{Entry{7, 3, "one.bin"}};
    ParserResultCatalogGeneration generation{};
    if (!build_catalog(h, entries, generation)) return fail("sink catalog");
    auto buffers = h.buffers();
    RecordingSink sink{2, 1};
    const auto started = h.session->begin_stream(
        generation, 7, sink, Clock::time_point::max());
    const std::array one{std::byte{0x41}};
    const std::array two{std::byte{0x42}, std::byte{0x43}};
    h.io.push_frame(MessageType::data_chunk, started.request_id, one);
    h.io.push_frame(MessageType::data_chunk, started.request_id, two);
    if (!receive(h, buffers).valid()) return fail("first sink chunk");
    const auto rejected = receive(h, buffers);
    if (rejected.error != ParserParentSessionError::result_failure ||
        sink.calls_ != 2 || sink.size_ != 2 || sink.bytes_[0] != one[0] ||
        sink.bytes_[1] != two[0] || !h.session->terminal())
      return fail("sink rejection partial effect failed");
  }
  {
    Harness h;
    const std::array entries{Entry{7, 1, "one.bin"}};
    ParserResultCatalogGeneration generation{};
    if (!build_catalog(h, entries, generation)) return fail("token catalog");
    RecordingSink sink;
    const auto unknown = h.session->begin_stream(
        generation, 8, sink, Clock::time_point::max());
    if (unknown.error != ParserParentSessionError::result_failure ||
        !h.session->terminal()) return fail("unknown token accepted");
  }
  return true;
}

[[nodiscard]] bool test_source_reads_and_tickets() {
  {
    Harness h;
    auto buffers = h.buffers();
    std::uint64_t request = 0;
    if (!begin_enum(h, request)) return fail("read enumeration");
    const auto first = read_request(1, kMarkerOffset, 1);
    h.io.push_frame(MessageType::read_request, request, first);
    const auto replied = receive(h, buffers);
    const auto header = h.io.write(3);
    const auto payload = h.io.write(4);
    if (replied.disposition != ParserParentSessionDisposition::read_replied ||
        header.size != 32 ||
        load_u16(header.prefix, 8) !=
            static_cast<std::uint16_t>(MessageType::read_reply) ||
        load_u64(header.prefix, 24) != request || payload.size != 7 ||
        load_u32(payload.prefix, 0) != 1 || load_u16(payload.prefix, 4) != 0 ||
        payload.prefix[6] != std::byte{0} || buffers.reply[6] != std::byte{0} ||
        buffers.scratch[0] != std::byte{0} ||
        !std::all_of(buffers.scratch.begin() + 1, buffers.scratch.end(),
                     [](auto byte) { return byte == std::byte{0xb6}; }))
      return fail("source reply wire/private storage retention");
    const auto second = read_request(2, kMarkerOffset + 1, 1);
    h.io.push_frame(MessageType::read_request, request, second);
    if (receive(h, buffers).disposition !=
        ParserParentSessionDisposition::read_replied)
      return fail("reply was not committed after exact send");
    std::fill(buffers.reply.begin(), buffers.reply.end(), std::byte{0});
  }
  {
    Harness h;
    auto buffers = h.buffers();
    std::uint64_t request = 0;
    if (!begin_enum(h, request)) return fail("abandon enumeration");
    h.io.set_write_failure(4, 2, IsolatedWorkerError::peer_closed);
    const auto read = read_request(1, kMarkerOffset, 1);
    h.io.push_frame(MessageType::read_request, request, read);
    const auto abandoned = receive(h, buffers);
    if (abandoned.error != ParserParentSessionError::channel_failure ||
        abandoned.source_result.error !=
            ParserSourceReadBrokerError::transport_abandoned ||
        buffers.reply[6] != std::byte{0} || !h.session->terminal())
      return fail("failed reply not abandoned/private storage lost");
    std::fill(buffers.reply.begin(), buffers.reply.end(), std::byte{0});
  }
  {
    Harness h;
    auto buffers = h.buffers();
    std::uint64_t request = 0;
    if (!begin_enum(h, request)) return fail("sequence enumeration");
    const auto wrong = read_request(2, kMarkerOffset, 1);
    h.io.push_frame(MessageType::read_request, request, wrong);
    if (receive(h, buffers).error != ParserParentSessionError::source_failure)
      return fail("wrong read sequence accepted");
  }
  {
    Harness h;
    auto buffers = h.buffers();
    std::uint64_t request = 0;
    if (!begin_enum(h, request)) return fail("maximum enumeration");
    const auto too_large = read_request(1, kMarkerOffset, 5);
    h.io.push_frame(MessageType::read_request, request, too_large);
    if (receive(h, buffers).error != ParserParentSessionError::source_failure)
      return fail("maximum read exceeded");
  }
  {
    Harness h;
    auto buffers = h.buffers();
    std::uint64_t request = 0;
    if (!begin_enum(h, request) ||
        !h.media.overwrite_byte(kMarkerOffset, std::byte{0xaa}, false))
      return fail("mutation fixture");
    const auto read = read_request(1, kMarkerOffset, 1);
    h.io.push_frame(MessageType::read_request, request, read);
    const auto changed = receive(h, buffers);
    if (changed.error != ParserParentSessionError::source_failure ||
        changed.source_result.error != ParserSourceReadBrokerError::source_changed)
      return fail("source mutation not retired");
  }
  return true;
}

[[nodiscard]] bool test_cancel_crossings_and_concurrency() {
  {
    Harness h;
    auto buffers = h.buffers();
    std::uint64_t request = 0;
    if (!begin_enum(h, request)) return fail("cancel begin");
    h.io.block_reads_when_empty();
    ParserParentSessionResult received;
    std::thread reader{[&] { received = receive(h, buffers); }};
    if (!h.io.wait_for_reads(2)) {
      h.session->notify_worker_failed();
      reader.join();
      return fail("blocked receive did not enter");
    }
    const auto reads = h.io.read_calls();
    auto duplicate_buffers = h.buffers();
    const auto duplicate = receive(h, duplicate_buffers);
    const auto shutdown = h.session->shutdown(Clock::time_point::max());
    const auto cancel = h.session->request_cancel(Clock::time_point::max());
    h.io.push_frame(MessageType::cancel_ack, request);
    reader.join();
    if (duplicate.error != ParserParentSessionError::concurrent_operation ||
        shutdown.error != ParserParentSessionError::concurrent_operation ||
        !cancel.valid() || h.io.read_calls() != reads ||
        received.disposition !=
            ParserParentSessionDisposition::cancellation_acknowledged ||
        h.session->state() != ParserParentSessionState::cancelled)
      return fail("blocked cancel/duplicate concurrency failed");
    const auto writes = h.io.write_calls();
    if (h.session->request_cancel(Clock::time_point::max()).error !=
            ParserParentSessionError::invalid_state ||
        h.io.write_calls() != writes ||
        h.session->shutdown(Clock::time_point::max()).disposition !=
            ParserParentSessionDisposition::shutdown_sent)
      return fail("cancel acknowledgement shutdown ordering failed");
  }
  {
    Harness h;
    auto buffers = h.buffers();
    std::uint64_t request = 0;
    if (!begin_enum(h, request) ||
        !h.session->request_cancel(Clock::time_point::max()).valid())
      return fail("complete crossing setup");
    constexpr auto complete = complete_payload();
    h.io.push_frame(MessageType::complete, request, complete);
    const auto crossed = receive(h, buffers);
    if (crossed.disposition !=
            ParserParentSessionDisposition::enumeration_complete ||
        h.session->state() != ParserParentSessionState::idle)
      return fail("complete did not win cancel crossing");
  }
  {
    Harness h;
    auto buffers = h.buffers();
    std::uint64_t request = 0;
    if (!begin_enum(h, request) ||
        !h.session->request_cancel(Clock::time_point::max()).valid())
      return fail("post-cancel read setup");
    const auto writes = h.io.write_calls();
    const auto read = read_request(1, kMarkerOffset, 1);
    h.io.push_frame(MessageType::read_request, request, read);
    if (receive(h, buffers).disposition !=
            ParserParentSessionDisposition::read_ignored_after_cancel ||
        h.io.write_calls() != writes) return fail("post-cancel read replied");
    h.io.push_frame(MessageType::cancel_ack, request);
    if (receive(h, buffers).disposition !=
        ParserParentSessionDisposition::cancellation_acknowledged)
      return fail("post-cancel acknowledgement failed");
  }
  return true;
}

[[nodiscard]] bool test_prompt_notifications_during_blocked_io() {
  {
    Harness h;
    ParserResultCatalogGeneration prior{};
    const std::array entry{Entry{1, 0, "a"}};
    if (!build_catalog(h, entry, prior)) return fail("blocked enumerate catalog");
    h.io.block_write(3);
    h.io.reenter_from_callbacks(*h.session, 3, true);
    ParserParentSessionResult operation;
    std::thread worker{[&] {
      operation = h.session->begin_enumeration(Clock::time_point::max());
    }};
    if (!h.io.wait_for_writes(4) || !h.io.wait_for_visibility()) {
      h.session->notify_worker_failed();
      worker.join();
      return fail("blocked enumerate did not enter");
    }
    const auto visible = h.io.visibility();
    const auto writes = h.io.write_calls();
    const auto competing =
        h.session->begin_enumeration(Clock::time_point::max());
    h.session->notify_worker_failed();
    worker.join();
    if (competing.error != ParserParentSessionError::concurrent_operation ||
        h.io.write_calls() != writes ||
        visible.state != ParserParentSessionState::idle ||
        visible.result_error != ParserParentSessionError::none ||
        visible.catalog_visible ||
        operation.error != ParserParentSessionError::worker_failure ||
        h.session->result().error != ParserParentSessionError::worker_failure ||
        h.io.abort_calls() != 1 || !h.io.callbacks_reentered())
      return fail("blocked enumerate notification/reentry failed");
  }
  {
    Harness h;
    ParserResultCatalogGeneration generation{};
    const std::array entry{Entry{1, 0, "a"}};
    if (!build_catalog(h, entry, generation)) return fail("blocked stream catalog");
    h.io.block_write(3);
    h.io.reenter_from_callbacks(*h.session, 3, false);
    RecordingSink sink;
    ParserParentSessionResult operation;
    std::thread worker{[&] {
      operation = h.session->begin_stream(generation, 1, sink,
                                          Clock::time_point::max());
    }};
    if (!h.io.wait_for_writes(4) || !h.io.wait_for_visibility()) {
      h.session->invalidate_source();
      worker.join();
      return fail("blocked stream did not enter");
    }
    const auto visible = h.io.visibility();
    const auto writes = h.io.write_calls();
    const auto competing = h.session->begin_stream(
        generation, 1, sink, Clock::time_point::max());
    h.session->invalidate_source();
    worker.join();
    if (competing.error != ParserParentSessionError::concurrent_operation ||
        h.io.write_calls() != writes ||
        visible.state != ParserParentSessionState::idle ||
        visible.result_error != ParserParentSessionError::none ||
        visible.catalog_visible ||
        operation.error != ParserParentSessionError::source_invalidated ||
        h.io.abort_calls() != 1)
      return fail("blocked stream invalidation failed");
  }
  {
    Harness h;
    auto buffers = h.buffers();
    std::uint64_t request = 0;
    if (!begin_enum(h, request)) return fail("blocked reply begin");
    const auto read = read_request(1, kMarkerOffset, 1);
    h.io.push_frame(MessageType::read_request, request, read);
    h.io.block_write(3);
    h.io.reenter_from_callbacks(*h.session, 3, false);
    ParserParentSessionResult operation;
    std::thread worker{[&] { operation = receive(h, buffers); }};
    if (!h.io.wait_for_writes(4) || !h.io.wait_for_visibility()) {
      h.session->notify_worker_failed();
      worker.join();
      return fail("blocked reply did not enter");
    }
    const auto visible = h.io.visibility();
    const auto writes = h.io.write_calls();
    auto other_buffers = h.buffers();
    const auto competing = receive(h, other_buffers);
    h.session->notify_worker_failed();
    worker.join();
    if (competing.error != ParserParentSessionError::concurrent_operation ||
        h.io.write_calls() != writes ||
        visible.state != ParserParentSessionState::enumerating ||
        visible.result_error != ParserParentSessionError::none ||
        visible.catalog_visible ||
        operation.error != ParserParentSessionError::worker_failure ||
        h.io.abort_calls() != 1)
      return fail("blocked reply notification failed");
  }
  {
    Harness h;
    std::uint64_t request = 0;
    if (!begin_enum(h, request)) return fail("blocked cancel begin");
    h.io.block_write(3);
    h.io.reenter_from_callbacks(*h.session, 3, false);
    ParserParentSessionResult operation;
    std::thread worker{[&] {
      operation = h.session->request_cancel(Clock::time_point::max());
    }};
    if (!h.io.wait_for_writes(4) || !h.io.wait_for_visibility()) {
      h.session->notify_worker_failed();
      worker.join();
      return fail("blocked cancel did not enter");
    }
    const auto visible = h.io.visibility();
    const auto writes = h.io.write_calls();
    const auto competing =
        h.session->request_cancel(Clock::time_point::max());
    h.session->notify_worker_failed();
    worker.join();
    if (competing.error != ParserParentSessionError::concurrent_operation ||
        h.io.write_calls() != writes ||
        visible.state != ParserParentSessionState::enumerating ||
        visible.result_error != ParserParentSessionError::none ||
        visible.catalog_visible ||
        operation.error != ParserParentSessionError::worker_failure ||
        h.io.abort_calls() != 1)
      return fail("blocked cancel notification failed");
  }
  {
    Harness h;
    ParserResultCatalogGeneration prior{};
    const std::array entry{Entry{1, 0, "a"}};
    if (!build_catalog(h, entry, prior)) return fail("blocked shutdown catalog");
    h.io.block_write(3);
    h.io.reenter_from_callbacks(*h.session, 3, false);
    ParserParentSessionResult operation;
    std::thread worker{[&] {
      operation = h.session->shutdown(Clock::time_point::max());
    }};
    if (!h.io.wait_for_writes(4) || !h.io.wait_for_visibility()) {
      h.session->invalidate_source();
      worker.join();
      return fail("blocked shutdown did not enter");
    }
    const auto visible = h.io.visibility();
    const auto writes = h.io.write_calls();
    const auto competing = h.session->shutdown(Clock::time_point::max());
    h.session->invalidate_source();
    worker.join();
    if (competing.error != ParserParentSessionError::concurrent_operation ||
        h.io.write_calls() != writes ||
        visible.state != ParserParentSessionState::idle ||
        visible.result_error != ParserParentSessionError::none ||
        visible.catalog_visible ||
        operation.error != ParserParentSessionError::source_invalidated ||
        h.io.abort_calls() != 1)
      return fail("blocked shutdown invalidation failed");
  }
  {
    Harness h;
    auto buffers = h.buffers();
    std::uint64_t request = 0;
    if (!begin_enum(h, request)) return fail("blocked receive notify begin");
    h.io.block_reads_when_empty();
    ParserParentSessionResult operation;
    std::thread worker{[&] { operation = receive(h, buffers); }};
    if (!h.io.wait_for_reads(2)) {
      h.session->notify_worker_failed();
      worker.join();
      return fail("blocked receive notify did not enter");
    }
    h.session->notify_worker_failed();
    worker.join();
    if (operation.error != ParserParentSessionError::worker_failure ||
        h.io.abort_calls() != 1)
      return fail("blocked receive notification failed");
  }
  return true;
}

[[nodiscard]] bool test_buffers_budgets_and_retirement() {
  {
    Harness h;
    auto buffers = h.buffers();
    std::uint64_t request = 0;
    if (!begin_enum(h, request)) return fail("buffer begin");
    const auto reads = h.io.read_calls();
    auto short_receive = std::span<std::byte>{buffers.receive}.first(
        buffers.receive.size() - 1);
    if (h.session->receive_one(short_receive, buffers.scratch, buffers.reply,
                               Clock::time_point::max()).error !=
            ParserParentSessionError::output_too_small ||
        h.io.read_calls() != reads)
      return fail("short capacity consumed bytes");
    if (h.session->receive_one(
            buffers.receive,
            std::span<std::byte>{buffers.scratch}.first(
                buffers.scratch.size() - 1),
            buffers.reply, Clock::time_point::max()).error !=
            ParserParentSessionError::output_too_small ||
        h.session->receive_one(
            buffers.receive, buffers.scratch,
            std::span<std::byte>{buffers.reply}.first(buffers.reply.size() - 1),
            Clock::time_point::max()).error !=
            ParserParentSessionError::output_too_small ||
        h.io.read_calls() != reads)
      return fail("short scratch/reply capacity consumed bytes");
    std::vector<std::byte> overlap(ohl::parser::kMaximumFramePayloadBytes + 32);
    const auto receive_span = std::span<std::byte>{overlap}.first(
        ohl::parser::kMaximumFramePayloadBytes);
    const auto scratch_span = std::span<std::byte>{overlap}.subspan(1, 4);
    if (h.session->receive_one(receive_span, scratch_span, buffers.reply,
                               Clock::time_point::max()).error !=
            ParserParentSessionError::overlapping_buffers ||
        h.io.read_calls() != reads)
      return fail("overlap consumed bytes");
    const auto reply_in_receive =
        std::span<std::byte>{overlap}.subspan(2, buffers.reply.size());
    if (h.session->receive_one(receive_span, buffers.scratch, reply_in_receive,
                               Clock::time_point::max()).error !=
            ParserParentSessionError::overlapping_buffers)
      return fail("receive/reply overlap accepted");
    std::vector<std::byte> scratch_reply(32);
    const auto disjoint_receive = std::span<std::byte>{buffers.receive};
    const auto overlapping_scratch =
        std::span<std::byte>{scratch_reply}.first(4);
    const auto overlapping_reply = std::span<std::byte>{scratch_reply}.subspan(
        1, buffers.reply.size());
    if (h.session->receive_one(disjoint_receive, overlapping_scratch,
                               overlapping_reply,
                               Clock::time_point::max()).error !=
            ParserParentSessionError::overlapping_buffers ||
        h.io.read_calls() != reads)
      return fail("scratch/reply overlap accepted");
    const auto dirty_header = wire_header(MessageType::entry_batch, 4, request);
    const std::array dirty{std::byte{0x11}, std::byte{0x22}, std::byte{0x33},
                           std::byte{0x44}};
    h.io.push_read(dirty_header);
    h.io.push_read(dirty, 2, IsolatedWorkerError::peer_closed);
    const auto tainted = receive(h, buffers);
    if (tainted.error != ParserParentSessionError::channel_failure ||
        buffers.receive[0] != dirty[0] || buffers.receive[1] != dirty[1] ||
        !std::all_of(buffers.receive.begin() + 2, buffers.receive.end(),
                     [](auto byte) { return byte == std::byte{0xa5}; }))
      return fail("partial payload taint contract failed");
  }
  {
    Harness h{small_read_limits(),
              {.maximum_messages = 2, .maximum_payload_bytes = 12}};
    const auto result = h.session->begin_enumeration(Clock::time_point::max());
    if (result.error != ParserParentSessionError::protocol_failure ||
        !h.session->terminal() || h.io.write_calls() != 2)
      return fail("tight protocol budget not enforced");
  }
  {
    PayloadImportLimits imports{};
    imports.maximum_entries = 1;
    Harness h{small_read_limits(), {}, imports};
    auto buffers = h.buffers();
    std::uint64_t request = 0;
    if (!begin_enum(h, request)) return fail("catalog budget begin");
    const std::array entries{Entry{1, 0, "a"}, Entry{2, 0, "b"}};
    const auto payload = entry_batch(entries);
    h.io.push_frame(MessageType::entry_batch, request, payload);
    if (receive(h, buffers).error != ParserParentSessionError::protocol_failure)
      return fail("catalog entry budget not enforced");
  }
  {
    Harness h{small_read_limits(1)};
    auto buffers = h.buffers();
    std::uint64_t request = 0;
    if (!begin_enum(h, request)) return fail("request budget begin");
    auto first = read_request(1, kMarkerOffset, 1);
    h.io.push_frame(MessageType::read_request, request, first);
    if (!receive(h, buffers).valid()) return fail("request budget first");
    auto second = read_request(2, kMarkerOffset, 1);
    h.io.push_frame(MessageType::read_request, request, second);
    const auto exceeded = receive(h, buffers);
    if (exceeded.source_result.error !=
        ParserSourceReadBrokerError::request_budget_exceeded)
      return fail("request budget not enforced");
  }
  {
    Harness h{small_read_limits(16, 10)};
    auto buffers = h.buffers();
    std::uint64_t request = 0;
    if (!begin_enum(h, request)) return fail("byte budget begin");
    auto first = read_request(1, kMarkerOffset, 1);
    h.io.push_frame(MessageType::read_request, request, first);
    if (!receive(h, buffers).valid()) return fail("byte budget first");
    auto second = read_request(2, kMarkerOffset, 1);
    h.io.push_frame(MessageType::read_request, request, second);
    if (receive(h, buffers).source_result.error !=
        ParserSourceReadBrokerError::byte_budget_exceeded)
      return fail("reply byte budget not enforced");
  }
  {
    Harness worker;
    ParserResultCatalogGeneration generation{};
    const std::array entry{Entry{1, 0, "a"}};
    if (!build_catalog(worker, entry, generation)) return fail("retire catalog");
    worker.session->notify_worker_failed();
    const auto aborts = worker.io.abort_calls();
    worker.session->invalidate_source();
    if (!worker.session->terminal() || worker.session->catalog().has_value() ||
        worker.session->result().error !=
            ParserParentSessionError::worker_failure ||
        worker.io.abort_calls() != aborts) return fail("worker retirement");
  }
  {
    Harness source;
    source.session->invalidate_source();
    if (!source.session->terminal() ||
        source.session->result().error !=
            ParserParentSessionError::source_invalidated ||
        source.io.abort_calls() != 1) return fail("source invalidation");
  }
  return true;
}

}  // namespace

int main() {
  const bool ok = test_construction_contract() &&
                  test_sticky_send_failures_and_destructor() &&
                  test_enumeration_catalog_and_streams() &&
                  test_sink_rejection_and_stale_token() &&
                  test_source_reads_and_tickets() &&
                  test_cancel_crossings_and_concurrency() &&
                  test_prompt_notifications_during_blocked_io() &&
                  test_buffers_budgets_and_retirement();
  return ok ? 0 : 1;
}
