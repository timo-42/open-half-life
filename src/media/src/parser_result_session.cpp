#include "ohl/media/parser_result_session.hpp"

#include <algorithm>
#include <array>
#include <limits>
#include <new>
#include <string>
#include <type_traits>
#include <utility>
#include <vector>

namespace ohl::media {
namespace {

[[nodiscard]] bool limits_valid(const PayloadImportLimits& limits) noexcept {
  return limits.maximum_entries != 0 &&
         limits.maximum_entries <= parser::kMaximumEnumeratedEntries &&
         limits.maximum_path_bytes != 0 &&
         limits.maximum_path_bytes <= parser::kMaximumEnumeratedPathBytes &&
         limits.maximum_entry_bytes != 0 &&
         limits.maximum_entry_bytes <= parser::kMaximumEnumeratedEntryBytes &&
         limits.maximum_total_bytes != 0 &&
         limits.maximum_total_bytes <= parser::kMaximumEnumeratedTotalBytes &&
         limits.maximum_entry_bytes <= limits.maximum_total_bytes;
}

}  // namespace

ParserResultSession::ParserResultSession(
    parser::ProtocolStateValidator&& protocol,
    const std::uint64_t worker_epoch,
    const PayloadImportLimits limits) noexcept
    : protocol_{std::move(protocol)},
      limits_{limits},
      worker_epoch_{worker_epoch} {
  if (protocol_.error() != parser::ProtocolError::none ||
      protocol_.state() != parser::SessionState::idle ||
      worker_epoch_ == 0 || !configuration_valid()) {
    failure_.error = ParserResultSessionError::invalid_configuration;
    failure_.protocol_error = protocol_.error();
    terminal_ = true;
  }
}

ParserResultSessionResult ParserResultSession::begin_enumeration(
    const parser::FrameView& frame) noexcept {
  if (terminal_) {
    return failure_;
  }
  const auto decoded = parser::decode_enumerate_payload(frame);
  if (!decoded.valid()) {
    return fail(ParserResultSessionError::protocol_failure, decoded.error);
  }
  if (enumeration_counter_ == std::numeric_limits<std::uint64_t>::max()) {
    return fail(ParserResultSessionError::generation_exhausted);
  }

  std::vector<PayloadEntryMetadata> prepared_entries;
  try {
    prepared_entries.reserve(limits_.maximum_entries);
  } catch (const std::bad_alloc&) {
    return fail(ParserResultSessionError::allocation_failure);
  } catch (...) {
    return fail(ParserResultSessionError::internal_failure);
  }

  const auto state_error = protocol_.observe(
      parser::MessageDirection::parent_to_worker, frame.header);
  if (state_error != parser::ProtocolError::none) {
    return fail(ParserResultSessionError::protocol_failure, state_error);
  }

  retire_all();
  ++enumeration_counter_;
  candidate_generation_ = {
      .worker_epoch = worker_epoch_,
      .enumeration = enumeration_counter_,
  };
  candidate_active_ = true;
  candidate_promotable_ = true;
  remaining_entries_ = static_cast<std::uint32_t>(limits_.maximum_entries);
  remaining_path_bytes_ = limits_.maximum_path_bytes;
  remaining_total_bytes_ = limits_.maximum_total_bytes;
  candidate_entries_.swap(prepared_entries);
  return succeed();
}

ParserResultSessionResult ParserResultSession::accept_entry_batch(
    const parser::FrameView& frame) noexcept {
  if (terminal_) {
    return failure_;
  }
  if (!candidate_active_) {
    return fail(ParserResultSessionError::invalid_state);
  }

  const parser::EntryBatchPolicy policy{
      .remaining_entries = remaining_entries_,
      .remaining_path_bytes = remaining_path_bytes_,
      .maximum_entry_bytes = limits_.maximum_entry_bytes,
      .remaining_total_bytes = remaining_total_bytes_,
      .has_previous_source_token = has_previous_source_token_,
      .previous_source_token = previous_source_token_,
  };
  std::array<parser::EntryBatchEntry,
             parser::kMaximumEntryBatchEntries>
      decoded_storage{};
  const auto decoded =
      parser::decode_entry_batch_payload(frame, policy, decoded_storage);
  if (!decoded.valid()) {
    return fail(ParserResultSessionError::protocol_failure, decoded.error);
  }

  std::uint64_t batch_path_bytes = 0;
  std::uint64_t batch_total_bytes = 0;
  for (const auto& entry : decoded.message.entries) {
    batch_path_bytes += static_cast<std::uint64_t>(entry.archive_path.size());
    batch_total_bytes += entry.size_bytes;
  }

  std::vector<PayloadEntryMetadata> prepared_entries;
  if (candidate_promotable_) {
    try {
      prepared_entries.reserve(decoded.message.entries.size());
      for (const auto& entry : decoded.message.entries) {
        prepared_entries.push_back(
            {.source_token = entry.source_token,
             .archive_path = std::string{entry.archive_path},
             .size_bytes = entry.size_bytes});
      }
    } catch (const std::bad_alloc&) {
      return fail(ParserResultSessionError::allocation_failure);
    } catch (...) {
      return fail(ParserResultSessionError::internal_failure);
    }
  }

  const auto state_error = protocol_.observe(
      parser::MessageDirection::worker_to_parent, frame.header);
  if (state_error != parser::ProtocolError::none) {
    return fail(ParserResultSessionError::protocol_failure, state_error);
  }

  if (candidate_promotable_) {
    static_assert(
        std::is_nothrow_move_constructible_v<PayloadEntryMetadata>);
    for (auto& entry : prepared_entries) {
      candidate_entries_.push_back(std::move(entry));
    }
  }
  remaining_entries_ -=
      static_cast<std::uint32_t>(decoded.message.entries.size());
  remaining_path_bytes_ -= batch_path_bytes;
  remaining_total_bytes_ -= batch_total_bytes;
  has_previous_source_token_ = true;
  previous_source_token_ = decoded.message.entries.back().source_token;
  return succeed();
}

ParserResultSessionResult ParserResultSession::complete_enumeration(
    const parser::FrameView& frame) noexcept {
  if (terminal_) {
    return failure_;
  }
  const auto decoded =
      parser::decode_complete_payload(frame, parser::ProtocolPhase::enumerate);
  if (!decoded.valid()) {
    return fail(ParserResultSessionError::protocol_failure, decoded.error);
  }
  if (!candidate_active_) {
    return fail(ParserResultSessionError::invalid_state);
  }

  PayloadLayout prepared_catalog;
  std::vector<TokenIndexEntry> prepared_index;
  if (candidate_promotable_) {
    try {
      prepared_catalog = plan_payload_layout(candidate_entries_, limits_);
      if (prepared_catalog.valid()) {
        prepared_index.reserve(prepared_catalog.entries.size());
        for (std::size_t index = 0;
             index < prepared_catalog.entries.size(); ++index) {
          prepared_index.push_back(
              {.source_token =
                   prepared_catalog.entries[index].source_token,
               .entry_index = index});
        }
        std::ranges::sort(prepared_index, {},
                          &TokenIndexEntry::source_token);
      }
    } catch (const std::bad_alloc&) {
      return fail(ParserResultSessionError::allocation_failure);
    } catch (...) {
      return fail(ParserResultSessionError::internal_failure);
    }

    const auto accepted_total = limits_.maximum_total_bytes -
                                remaining_total_bytes_;
    if (!prepared_catalog.valid()) {
      return fail_layout(prepared_catalog);
    }
    if (prepared_catalog.entries.size() != candidate_entries_.size() ||
        prepared_catalog.total_bytes != accepted_total ||
        prepared_index.size() != candidate_entries_.size()) {
      return fail(ParserResultSessionError::result_validation_failure);
    }
    for (std::size_t index = 1; index < prepared_index.size(); ++index) {
      if (prepared_index[index - 1].source_token >=
          prepared_index[index].source_token) {
        return fail(ParserResultSessionError::result_validation_failure);
      }
    }
  }

  const auto state_error = protocol_.observe(
      parser::MessageDirection::worker_to_parent, frame.header);
  if (state_error != parser::ProtocolError::none) {
    return fail(ParserResultSessionError::protocol_failure, state_error);
  }

  if (candidate_promotable_) {
    static_assert(std::is_nothrow_move_assignable_v<PayloadLayout>);
    retire_catalog();
    catalog_ = std::move(prepared_catalog);
    token_index_.swap(prepared_index);
    catalog_generation_ = candidate_generation_;
  }
  clear_candidate();
  return succeed();
}

ParserResultSessionResult ParserResultSession::begin_stream_entry(
    const parser::FrameView& frame,
    const ParserResultCatalogGeneration expected_generation) noexcept {
  if (terminal_) {
    return failure_;
  }
  const auto decoded = parser::decode_stream_entry_payload(frame);
  if (!decoded.valid()) {
    return fail(ParserResultSessionError::protocol_failure, decoded.error);
  }
  if (!catalog_generation_.valid() ||
      expected_generation != catalog_generation_) {
    return fail(ParserResultSessionError::unknown_source_token);
  }
  const auto* entry = find_catalog_entry(decoded.message.source_token);
  if (entry == nullptr) {
    return fail(ParserResultSessionError::unknown_source_token);
  }
  const auto entry_size = entry->size_bytes;

  const auto state_error = protocol_.observe(
      parser::MessageDirection::parent_to_worker, frame.header);
  if (state_error != parser::ProtocolError::none) {
    return fail(ParserResultSessionError::protocol_failure, state_error);
  }

  stream_active_ = true;
  stream_generation_ = catalog_generation_;
  stream_source_token_ = decoded.message.source_token;
  remaining_stream_bytes_ = entry_size;
  return succeed();
}

ParserResultSessionResult ParserResultSession::accept_data_chunk(
    const parser::FrameView& frame,
    PayloadByteSink& destination) noexcept {
  if (terminal_) {
    return failure_;
  }
  if (!stream_active_) {
    return fail(ParserResultSessionError::invalid_state);
  }
  const auto decoded =
      parser::decode_data_chunk_payload(frame, remaining_stream_bytes_);
  if (!decoded.valid()) {
    return fail(ParserResultSessionError::protocol_failure, decoded.error);
  }

  const auto state_error = protocol_.observe(
      parser::MessageDirection::worker_to_parent, frame.header);
  if (state_error != parser::ProtocolError::none) {
    return fail(ParserResultSessionError::protocol_failure, state_error);
  }
  if (!destination.write(decoded.message.data)) {
    return fail(ParserResultSessionError::downstream_failure);
  }
  remaining_stream_bytes_ -=
      static_cast<std::uint64_t>(decoded.message.data.size());
  return succeed();
}

ParserResultSessionResult ParserResultSession::complete_stream(
    const parser::FrameView& frame) noexcept {
  if (terminal_) {
    return failure_;
  }
  const auto decoded =
      parser::decode_complete_payload(frame, parser::ProtocolPhase::stream);
  if (!decoded.valid()) {
    return fail(ParserResultSessionError::protocol_failure, decoded.error);
  }
  if (!stream_active_) {
    return fail(ParserResultSessionError::invalid_state);
  }
  if (remaining_stream_bytes_ != 0) {
    return fail(ParserResultSessionError::incomplete_stream);
  }

  const auto state_error = protocol_.observe(
      parser::MessageDirection::worker_to_parent, frame.header);
  if (state_error != parser::ProtocolError::none) {
    return fail(ParserResultSessionError::protocol_failure, state_error);
  }
  clear_stream();
  return succeed();
}

ParserReadRequestResult ParserResultSession::accept_read_request(
    const parser::FrameView& frame,
    const parser::SourceReadPolicy& policy,
    const std::uint32_t expected_sequence) noexcept {
  if (terminal_) {
    return {.result = failure_,
            .disposition = ParserReadRequestDisposition::unavailable,
            .message = {}};
  }
  const auto disposition =
      protocol_.state() == parser::SessionState::cancelling
          ? ParserReadRequestDisposition::ignored_after_cancel
          : ParserReadRequestDisposition::serviceable;
  const auto decoded =
      parser::decode_read_request_payload(frame, policy, expected_sequence);
  if (!decoded.valid()) {
    return {.result = fail(ParserResultSessionError::protocol_failure,
                           decoded.error),
            .disposition = ParserReadRequestDisposition::unavailable,
            .message = {}};
  }
  const auto state_error = protocol_.observe(
      parser::MessageDirection::worker_to_parent, frame.header);
  if (state_error != parser::ProtocolError::none) {
    return {.result = fail(ParserResultSessionError::protocol_failure,
                           state_error),
            .disposition = ParserReadRequestDisposition::unavailable,
            .message = {}};
  }
  if (disposition == ParserReadRequestDisposition::ignored_after_cancel) {
    return {.result = {}, .disposition = disposition, .message = {}};
  }
  return {.result = {},
          .disposition = disposition,
          .message = decoded.message};
}

ParserResultSessionResult ParserResultSession::accept_read_reply(
    const parser::FrameView& frame, const std::uint32_t expected_sequence,
    const std::uint32_t requested_length) noexcept {
  if (terminal_) {
    return failure_;
  }
  const auto decoded = parser::decode_read_reply_payload(
      frame, expected_sequence, requested_length);
  if (!decoded.valid()) {
    return fail(ParserResultSessionError::protocol_failure, decoded.error);
  }
  const auto state_error = protocol_.observe(
      parser::MessageDirection::parent_to_worker, frame.header);
  if (state_error != parser::ProtocolError::none) {
    return fail(ParserResultSessionError::protocol_failure, state_error);
  }
  if (decoded.message.status == parser::ProtocolStatus::source_changed) {
    return fail(ParserResultSessionError::source_invalidated);
  }
  if (decoded.message.status == parser::ProtocolStatus::source_read_failed) {
    return fail(ParserResultSessionError::source_read_failure);
  }
  return succeed();
}

ParserResultSessionResult ParserResultSession::accept_cancel(
    const parser::FrameView& frame) noexcept {
  if (terminal_) {
    return failure_;
  }
  const auto decoded = parser::decode_cancel_payload(frame);
  if (!decoded.valid()) {
    return fail(ParserResultSessionError::protocol_failure, decoded.error);
  }
  const auto state_error = protocol_.observe(
      parser::MessageDirection::parent_to_worker, frame.header);
  if (state_error != parser::ProtocolError::none) {
    return fail(ParserResultSessionError::protocol_failure, state_error);
  }
  retire_for_cancel();
  return succeed();
}

ParserResultSessionResult ParserResultSession::accept_cancel_ack(
    const parser::FrameView& frame) noexcept {
  if (terminal_) {
    return failure_;
  }
  const auto decoded = parser::decode_cancel_ack_payload(frame);
  if (!decoded.valid()) {
    return fail(ParserResultSessionError::protocol_failure, decoded.error);
  }
  const auto state_error = protocol_.observe(
      parser::MessageDirection::worker_to_parent, frame.header);
  if (state_error != parser::ProtocolError::none) {
    return fail(ParserResultSessionError::protocol_failure, state_error);
  }
  retire_all();
  return succeed();
}

ParserResultSessionResult ParserResultSession::accept_shutdown(
    const parser::FrameView& frame) noexcept {
  if (terminal_) {
    return failure_;
  }
  const auto decoded = parser::decode_shutdown_payload(frame);
  if (!decoded.valid()) {
    return fail(ParserResultSessionError::protocol_failure, decoded.error);
  }
  const auto state_error = protocol_.observe(
      parser::MessageDirection::parent_to_worker, frame.header);
  if (state_error != parser::ProtocolError::none) {
    return fail(ParserResultSessionError::protocol_failure, state_error);
  }
  retire_all();
  return succeed();
}

void ParserResultSession::invalidate_source() noexcept {
  if (!terminal_) {
    (void)fail(ParserResultSessionError::source_invalidated);
  }
}

void ParserResultSession::worker_failed() noexcept {
  if (!terminal_) {
    (void)fail(ParserResultSessionError::worker_failure);
  }
}

ParserResultSessionResult ParserResultSession::result() const noexcept {
  return terminal_ ? failure_ : succeed();
}

std::optional<ParserResultCatalogView> ParserResultSession::catalog()
    const noexcept {
  if (terminal_ || !catalog_generation_.valid()) {
    return std::nullopt;
  }
  return ParserResultCatalogView{
      .generation = catalog_generation_,
      .entries = catalog_.entries,
      .total_bytes = catalog_.total_bytes,
  };
}

ParserResultSessionResult ParserResultSession::succeed() const noexcept {
  return {};
}

ParserResultSessionResult ParserResultSession::fail(
    const ParserResultSessionError error,
    const parser::ProtocolError protocol_error) noexcept {
  if (!terminal_) {
    failure_.error = error;
    failure_.protocol_error = protocol_error;
    terminal_ = true;
    retire_all();
  }
  return failure_;
}

ParserResultSessionResult ParserResultSession::fail_layout(
    const PayloadLayout& layout) noexcept {
  if (!terminal_) {
    failure_.error = ParserResultSessionError::result_validation_failure;
    failure_.layout_error = layout.error;
    failure_.path_error = layout.path_error;
    failure_.rejected_entry = layout.rejected_entry;
    terminal_ = true;
    retire_all();
  }
  return failure_;
}

bool ParserResultSession::configuration_valid() const noexcept {
  return limits_valid(limits_);
}

const PlannedPayloadEntry* ParserResultSession::find_catalog_entry(
    const std::uint64_t source_token) const noexcept {
  const auto found = std::ranges::lower_bound(
      token_index_, source_token, {}, &TokenIndexEntry::source_token);
  if (found == token_index_.end() || found->source_token != source_token ||
      found->entry_index >= catalog_.entries.size()) {
    return nullptr;
  }
  return &catalog_.entries[found->entry_index];
}

void ParserResultSession::retire_catalog() noexcept {
  catalog_generation_ = {};
  catalog_ = {};
  token_index_.clear();
}

void ParserResultSession::clear_candidate() noexcept {
  candidate_generation_ = {};
  candidate_active_ = false;
  candidate_promotable_ = false;
  remaining_entries_ = 0;
  remaining_path_bytes_ = 0;
  remaining_total_bytes_ = 0;
  has_previous_source_token_ = false;
  previous_source_token_ = 0;
  candidate_entries_.clear();
}

void ParserResultSession::clear_stream() noexcept {
  stream_active_ = false;
  stream_generation_ = {};
  stream_source_token_ = 0;
  remaining_stream_bytes_ = 0;
}

void ParserResultSession::retire_all() noexcept {
  retire_catalog();
  clear_candidate();
  clear_stream();
}

void ParserResultSession::retire_for_cancel() noexcept {
  retire_catalog();
  if (candidate_active_) {
    candidate_promotable_ = false;
    candidate_entries_.clear();
  }
  // Keep candidate quotas and stream remainder only to validate bounded
  // same-request result frames already crossing in the duplex transport.
}

}  // namespace ohl::media
