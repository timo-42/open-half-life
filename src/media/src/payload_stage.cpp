#include "ohl/media/payload_stage.hpp"

#include "source_stability_internal.hpp"

#include "ohl/core/sha256.hpp"
#include "ohl/media/payload_layout.hpp"
#include "ohl/media/payload_path.hpp"

#include <array>
#include <cstddef>
#include <cstdint>
#include <memory>
#include <optional>
#include <span>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace ohl::media {
namespace {

constexpr std::string_view kIdentityDomain =
    "open-half-life-payload-stage";
constexpr std::uint64_t kIdentityVersion = 2;

struct PreparedPlan {
  std::string identity;
  std::vector<std::vector<std::string_view>> components;
  std::vector<platform::AtomicDirectoryEntry> store_entries;
};

void append_u64(ohl::core::Sha256& hash, const std::uint64_t value) noexcept {
  std::array<std::byte, 8> encoded{};
  for (std::size_t index = 0; index < encoded.size(); ++index) {
    encoded[encoded.size() - index - 1] =
        static_cast<std::byte>((value >> (index * 8U)) & 0xffU);
  }
  hash.update(encoded);
}

void append_string(ohl::core::Sha256& hash,
                   const std::string_view value) noexcept {
  append_u64(hash, static_cast<std::uint64_t>(value.size()));
  hash.update(std::as_bytes(std::span{value.data(), value.size()}));
}

[[nodiscard]] std::vector<std::string_view> split_components(
    const std::string_view path) {
  std::vector<std::string_view> result;
  std::size_t begin = 0;
  while (begin < path.size()) {
    const auto end = path.find('/', begin);
    result.push_back(path.substr(begin, end == std::string_view::npos
                                           ? std::string_view::npos
                                           : end - begin));
    if (end == std::string_view::npos) {
      break;
    }
    begin = end + 1;
  }
  return result;
}

[[nodiscard]] bool same_entry(const PlannedPayloadEntry& first,
                              const PlannedPayloadEntry& second) noexcept {
  return first.source_token == second.source_token &&
         first.relative_path == second.relative_path &&
         first.size_bytes == second.size_bytes;
}

[[nodiscard]] std::optional<PreparedPlan> prepare_plan(
    const ValidatedMedia& media, const PayloadStageRequest& request,
    std::optional<std::size_t>& failing_entry) {
  if (!media.valid() || media.source() == nullptr ||
      media.source()->size() != media.fingerprint().size_bytes ||
      request.recipe_identity.empty() ||
      request.recipe_identity.size() >
          kMaximumPayloadStageRecipeIdentityBytes) {
    return std::nullopt;
  }

  const auto& limits = request.limits;
  if (limits.maximum_entries == 0 || limits.maximum_path_bytes == 0 ||
      limits.maximum_entry_bytes == 0 || limits.maximum_total_bytes == 0 ||
      limits.maximum_entry_bytes > limits.maximum_total_bytes ||
      request.entries.size() > limits.maximum_entries) {
    return std::nullopt;
  }
  std::uint64_t total_path_bytes = 0;
  std::uint64_t total_bytes = 0;
  for (std::size_t index = 0; index < request.entries.size(); ++index) {
    const auto& entry = request.entries[index];
    failing_entry = index;
    if (std::cmp_greater(entry.relative_path.size(),
                         limits.maximum_path_bytes - total_path_bytes) ||
        entry.size_bytes > limits.maximum_entry_bytes ||
        entry.size_bytes > limits.maximum_total_bytes - total_bytes) {
      return std::nullopt;
    }
    total_path_bytes +=
        static_cast<std::uint64_t>(entry.relative_path.size());
    total_bytes += entry.size_bytes;
  }
  if (total_bytes != request.declared_total_bytes) {
    failing_entry.reset();
    return std::nullopt;
  }
  failing_entry.reset();

  std::vector<PayloadEntryMetadata> metadata;
  metadata.reserve(request.entries.size());
  for (const auto& entry : request.entries) {
    metadata.push_back({entry.source_token, entry.relative_path,
                        entry.size_bytes});
  }
  const auto planned = plan_payload_layout(metadata, limits);
  if (!planned.valid() || planned.total_bytes != request.declared_total_bytes ||
      planned.entries.size() != request.entries.size()) {
    failing_entry = planned.rejected_entry;
    return std::nullopt;
  }
  for (std::size_t index = 0; index < request.entries.size(); ++index) {
    if (!same_entry(planned.entries[index], request.entries[index])) {
      failing_entry = index;
      return std::nullopt;
    }
  }

  PreparedPlan prepared;
  prepared.components.reserve(request.entries.size());
  for (const auto& entry : request.entries) {
    prepared.components.push_back(split_components(entry.relative_path));
  }
  prepared.store_entries.reserve(request.entries.size());
  for (std::size_t index = 0; index < request.entries.size(); ++index) {
    prepared.store_entries.push_back(
        {prepared.components[index], request.entries[index].size_bytes});
  }

  ohl::core::Sha256 hash;
  append_string(hash, kIdentityDomain);
  append_u64(hash, kIdentityVersion);
  append_u64(hash, media.fingerprint().size_bytes);
  append_string(hash, media.fingerprint().sha256);
  append_string(hash, request.recipe_identity);
  append_u64(hash, static_cast<std::uint64_t>(request.entries.size()));
  append_u64(hash, request.declared_total_bytes);
  for (const auto& entry : request.entries) {
    append_string(hash, entry.relative_path);
    append_u64(hash, entry.size_bytes);
  }
  prepared.identity = "ohl-payload-v2-sha256:";
  prepared.identity += ohl::core::hex_encode(hash.finish());
  return prepared;
}

class PlatformSinkAdapter final : public PayloadByteSink {
 public:
  explicit PlatformSinkAdapter(
      platform::AtomicDirectoryByteSink& sink) noexcept
      : sink_(sink) {}

  [[nodiscard]] bool write(
      const std::span<const std::byte> bytes) noexcept override {
    const auto error = sink_.write(bytes);
    if (error != platform::AtomicDirectoryStoreError::none) {
      error_ = error;
      return false;
    }
    return true;
  }

  [[nodiscard]] platform::AtomicDirectoryStoreError error() const noexcept {
    return error_;
  }

 private:
  platform::AtomicDirectoryByteSink& sink_;
  platform::AtomicDirectoryStoreError error_{
      platform::AtomicDirectoryStoreError::none};
};

void abort_transaction(PayloadStageResult& result,
                       platform::AtomicDirectoryTransaction& transaction)
    noexcept {
  result.cleanup_attempted = true;
  result.cleanup_error = transaction.abort();
}

[[nodiscard]] PayloadStageVerificationError map_verification_error(
    const detail::SourceStabilityError error) noexcept {
  switch (error) {
    case detail::SourceStabilityError::none:
      return PayloadStageVerificationError::none;
    case detail::SourceStabilityError::invalid_capability:
      return PayloadStageVerificationError::invalid_capability;
    case detail::SourceStabilityError::source_changed:
      return PayloadStageVerificationError::source_changed;
    case detail::SourceStabilityError::read_failure:
      return PayloadStageVerificationError::read_failure;
    case detail::SourceStabilityError::digest_mismatch:
      return PayloadStageVerificationError::digest_mismatch;
    case detail::SourceStabilityError::cancelled:
      return PayloadStageVerificationError::cancelled;
  }
  return PayloadStageVerificationError::read_failure;
}

}  // namespace

PayloadStageResult stage_payload(const ValidatedMedia& media,
                                 const PayloadStageRequest& request,
                                 PayloadSource& source,
                                 platform::AtomicDirectoryStore& store,
                                 const std::stop_token stop_token) {
  PayloadStageResult result;
  auto prepared = prepare_plan(media, request, result.failing_entry);
  if (!prepared.has_value()) {
    if (!media.valid() || media.source() == nullptr ||
        media.source()->size() != media.fingerprint().size_bytes) {
      result.error = PayloadStageError::source_verification_failure;
      result.verification_error =
          PayloadStageVerificationError::invalid_capability;
    } else {
      result.error = PayloadStageError::invalid_request;
    }
    return result;
  }
  result.identity = prepared->identity;
  if (stop_token.stop_requested()) {
    result.phase = PayloadStagePhase::cancellation;
    result.error = PayloadStageError::cancelled;
    return result;
  }
  const platform::AtomicDirectoryPlan store_plan{
      result.identity, prepared->store_entries};

  const auto initial_probe = store.probe(store_plan);
  if (initial_probe.error != platform::AtomicDirectoryStoreError::none) {
    result.phase = PayloadStagePhase::probe;
    result.error = PayloadStageError::store_failure;
    result.store_error = initial_probe.error;
    return result;
  }
  if (initial_probe.state ==
      platform::AtomicDirectoryProbeState::matching) {
    result.status = PayloadStageStatus::cache_hit;
    result.phase = PayloadStagePhase::probe;
    return result;
  }
  if (initial_probe.state ==
      platform::AtomicDirectoryProbeState::conflict) {
    result.status = PayloadStageStatus::conflict;
    result.phase = PayloadStagePhase::probe;
    return result;
  }

  auto transaction_result = store.create_transaction();
  if (transaction_result.error != platform::AtomicDirectoryStoreError::none ||
      transaction_result.transaction == nullptr) {
    result.phase = PayloadStagePhase::create_transaction;
    result.error = PayloadStageError::store_failure;
    result.store_error =
        transaction_result.error != platform::AtomicDirectoryStoreError::none
            ? transaction_result.error
            : platform::AtomicDirectoryStoreError::invalid_state;
    return result;
  }
  auto& transaction = *transaction_result.transaction;

  const auto begin_error = transaction.begin(store_plan);
  if (begin_error != platform::AtomicDirectoryStoreError::none) {
    result.phase = PayloadStagePhase::begin;
    result.error = PayloadStageError::store_failure;
    result.store_error = begin_error;
    abort_transaction(result, transaction);
    return result;
  }

  for (std::size_t index = 0; index < request.entries.size(); ++index) {
    const auto& entry = request.entries[index];
    auto open_result = transaction.open_file(
        prepared->components[index], entry.size_bytes);
    if (open_result.error != platform::AtomicDirectoryStoreError::none ||
        open_result.sink == nullptr) {
      result.phase = PayloadStagePhase::open_file;
      result.error = PayloadStageError::store_failure;
      result.store_error =
          open_result.error != platform::AtomicDirectoryStoreError::none
              ? open_result.error
              : platform::AtomicDirectoryStoreError::invalid_state;
      result.failing_entry = index;
      open_result.sink.reset();
      abort_transaction(result, transaction);
      return result;
    }

    PayloadStreamResult stream_result;
    platform::AtomicDirectoryStoreError write_error{
        platform::AtomicDirectoryStoreError::none};
    {
      PlatformSinkAdapter sink_adapter(*open_result.sink);
      stream_result = stream_payload_entry(
          entry, *media.source(), source, stop_token, sink_adapter);
      write_error = sink_adapter.error();
    }
    result.bytes_streamed += stream_result.bytes_written;
    if (!stream_result.complete()) {
      result.phase = PayloadStagePhase::stream_file;
      result.error = stream_result.error == PayloadStreamError::cancelled
                         ? PayloadStageError::cancelled
                         : PayloadStageError::stream_failure;
      result.stream_error = stream_result.error;
      result.store_error = write_error;
      result.failing_entry = index;
      open_result.sink.reset();
      abort_transaction(result, transaction);
      return result;
    }
    ++result.entries_streamed;

    const auto seal_error =
        transaction.seal_file(std::move(open_result.sink));
    if (seal_error != platform::AtomicDirectoryStoreError::none) {
      result.phase = PayloadStagePhase::seal_file;
      result.error = PayloadStageError::store_failure;
      result.store_error = seal_error;
      result.failing_entry = index;
      abort_transaction(result, transaction);
      return result;
    }
  }

  const platform::AtomicDirectoryCompletion completion{
      result.identity, static_cast<std::uint64_t>(request.entries.size()),
      request.declared_total_bytes};
  const auto completion_error = transaction.seal_completion(completion);
  if (completion_error != platform::AtomicDirectoryStoreError::none) {
    result.phase = PayloadStagePhase::seal_completion;
    result.error = PayloadStageError::store_failure;
    result.store_error = completion_error;
    abort_transaction(result, transaction);
    return result;
  }

  const auto verification =
      detail::verify_complete_source_stability(media, stop_token);
  if (verification != detail::SourceStabilityError::none) {
    result.phase = PayloadStagePhase::verify_source;
    result.error = verification == detail::SourceStabilityError::cancelled
                       ? PayloadStageError::cancelled
                       : PayloadStageError::source_verification_failure;
    result.verification_error = map_verification_error(verification);
    abort_transaction(result, transaction);
    return result;
  }

  if (stop_token.stop_requested()) {
    result.phase = PayloadStagePhase::cancellation;
    result.error = PayloadStageError::cancelled;
    abort_transaction(result, transaction);
    return result;
  }

  const auto publish_result = transaction.publish_no_replace();
  if (publish_result.error != platform::AtomicDirectoryStoreError::none) {
    result.phase = PayloadStagePhase::publish;
    result.error = PayloadStageError::publish_failure;
    result.store_error = publish_result.error;
    abort_transaction(result, transaction);
    return result;
  }
  if (publish_result.state ==
      platform::AtomicDirectoryPublishState::destination_exists) {
    result.phase = PayloadStagePhase::revalidate;
    abort_transaction(result, transaction);
    const auto winner = store.probe(store_plan);
    if (winner.error != platform::AtomicDirectoryStoreError::none) {
      result.winner_observation = PayloadWinnerObservation::probe_failed;
      result.store_error = winner.error;
    } else if (winner.state ==
               platform::AtomicDirectoryProbeState::matching) {
      result.winner_observation = PayloadWinnerObservation::matching;
    } else if (winner.state ==
               platform::AtomicDirectoryProbeState::conflict) {
      result.winner_observation = PayloadWinnerObservation::conflict;
    } else {
      result.winner_observation = PayloadWinnerObservation::absent;
    }

    if (result.cleanup_error != platform::AtomicDirectoryStoreError::none) {
      result.status = PayloadStageStatus::cleanup_failed;
      result.error = PayloadStageError::cleanup_failure;
      return result;
    }
    if (result.winner_observation == PayloadWinnerObservation::matching) {
      result.status = PayloadStageStatus::cache_hit;
      return result;
    }
    if (result.winner_observation == PayloadWinnerObservation::conflict) {
      result.status = PayloadStageStatus::conflict;
      return result;
    }
    result.error = PayloadStageError::revalidation_failure;
    return result;
  }

  result.publication = PayloadPublicationState::published;
  const auto sync_error = transaction.sync_published_parent();
  if (sync_error != platform::AtomicDirectoryStoreError::none) {
    result.status = PayloadStageStatus::published_sync_uncertain;
    result.phase = PayloadStagePhase::sync_published_parent;
    result.error = PayloadStageError::published_sync_failure;
    result.publication =
        PayloadPublicationState::published_sync_uncertain;
    result.store_error = sync_error;
    return result;
  }

  result.status = PayloadStageStatus::published_sync_complete;
  result.phase = PayloadStagePhase::complete;
  result.publication = PayloadPublicationState::published_sync_complete;
  return result;
}

}  // namespace ohl::media
