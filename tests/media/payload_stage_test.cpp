#include "ohl/media/payload_stage.hpp"

#include "ohl/platform/atomic_directory_store.hpp"
#include "synthetic_media_test_support.hpp"

#include <cstddef>
#include <cstdint>
#include <functional>
#include <initializer_list>
#include <iostream>
#include <limits>
#include <memory>
#include <span>
#include <stop_token>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace {

using ohl::platform::AtomicDirectoryProbeState;
using ohl::platform::AtomicDirectoryStoreError;

const ohl::media::ValidatedMedia* default_media = nullptr;

[[nodiscard]] std::vector<std::byte> bytes(
    std::initializer_list<unsigned int> values) {
  std::vector<std::byte> result;
  result.reserve(values.size());
  for (const auto value : values) {
    result.push_back(static_cast<std::byte>(value));
  }
  return result;
}

struct SourceScript {
  std::uint64_t token{0};
  std::vector<std::vector<std::byte>> chunks;
  bool succeed{true};
};

class ScriptedSource final : public ohl::media::PayloadSource {
 public:
  explicit ScriptedSource(std::vector<SourceScript> scripts,
                          const std::stop_token expected_stop_token = {},
                          std::stop_source* request_stop = nullptr,
                          const std::size_t request_after_chunk = 0,
                          const bool request_before_return = false)
      : scripts_(std::move(scripts)),
        expected_stop_token_(expected_stop_token),
        request_stop_(request_stop),
        request_after_chunk_(request_after_chunk),
        request_before_return_(request_before_return) {}

  [[nodiscard]] bool stream(
      const ohl::platform::MediaSource& media_source,
      const std::uint64_t source_token, const std::stop_token stop_token,
      ohl::media::PayloadByteSink& sink) noexcept override {
    media_sources.push_back(&media_source);
    tokens.push_back(source_token);
    stop_tokens.push_back(stop_token);
    sinks.push_back(&sink);
    contract_ok = default_media != nullptr && default_media->source() != nullptr &&
                  &media_source == default_media->source().get() &&
                  stop_token == expected_stop_token_ && sinks.back() != nullptr;
    if (!contract_ok) {
      return false;
    }
    for (const auto& script : scripts_) {
      if (script.token != source_token) {
        continue;
      }
      for (std::size_t index = 0; index < script.chunks.size(); ++index) {
        if (!sink.write(script.chunks[index])) {
          return false;
        }
        if (request_stop_ != nullptr && request_after_chunk_ == index + 1) {
          (void)request_stop_->request_stop();
        }
      }
      if (request_stop_ != nullptr && request_before_return_) {
        (void)request_stop_->request_stop();
      }
      return script.succeed;
    }
    return false;
  }

  std::vector<std::uint64_t> tokens;
  std::vector<const ohl::platform::MediaSource*> media_sources;
  std::vector<std::stop_token> stop_tokens;
  std::vector<const ohl::media::PayloadByteSink*> sinks;
  bool contract_ok{true};

 private:
  std::vector<SourceScript> scripts_;
  std::stop_token expected_stop_token_;
  std::stop_source* request_stop_{nullptr};
  std::size_t request_after_chunk_{0};
  bool request_before_return_{false};
};

struct FailureScript {
  std::size_t begin_call{0};
  std::size_t open_call{0};
  std::size_t write_call{0};
  std::size_t seal_call{0};
  std::size_t completion_call{0};
  std::size_t publish_call{0};
  std::size_t sync_call{0};
  std::size_t abort_call{0};
  bool publish_lost_race{false};
  bool create_failure{false};
  std::function<void()> after_completion;
};

struct OpenObservation {
  std::vector<std::string> components;
  std::uint64_t expected_size{0};
};

struct StoredEntry {
  std::vector<std::string> components;
  std::uint64_t expected_size{0};
  std::vector<std::byte> data;
};

class FakeStore;

class FakeSink final : public ohl::platform::AtomicDirectoryByteSink {
 public:
  FakeSink(FakeStore& store, bool& open,
           std::vector<std::byte>& staged_data) noexcept
      : store_(store), open_(open), staged_data_(staged_data) {}
  ~FakeSink() override;

  [[nodiscard]] AtomicDirectoryStoreError write(
      std::span<const std::byte> data) noexcept override;

 private:
  FakeStore& store_;
  bool& open_;
  std::vector<std::byte>& staged_data_;
};

class FakeTransaction final : public ohl::platform::AtomicDirectoryTransaction {
 public:
  explicit FakeTransaction(FakeStore& store) noexcept : store_(store) {}

  [[nodiscard]] AtomicDirectoryStoreError begin(
      const ohl::platform::AtomicDirectoryPlan& plan) noexcept override;
  [[nodiscard]] ohl::platform::AtomicDirectoryOpenResult open_file(
      std::span<const std::string_view> components,
      std::uint64_t expected_size) noexcept override;
  [[nodiscard]] AtomicDirectoryStoreError seal_file(
      std::unique_ptr<ohl::platform::AtomicDirectoryByteSink> sink)
      noexcept override;
  [[nodiscard]] AtomicDirectoryStoreError seal_completion(
      const ohl::platform::AtomicDirectoryCompletion& completion)
      noexcept override;
  [[nodiscard]] ohl::platform::AtomicDirectoryPublishResult
  publish_no_replace() noexcept override;
  [[nodiscard]] AtomicDirectoryStoreError sync_published_parent()
      noexcept override;
  [[nodiscard]] AtomicDirectoryStoreError abort() noexcept override;

 private:
  enum class State { created, staging, published, aborted };

  FakeStore& store_;
  State state_{State::created};
  bool open_{false};
  bool completion_sealed_{false};
  std::size_t sealed_entries_{0};
  std::string plan_identity_;
  std::vector<StoredEntry> staged_entries_;
};

class FakeStore final : public ohl::platform::AtomicDirectoryStore {
 public:
  [[nodiscard]] ohl::platform::AtomicDirectoryProbeResult probe(
      const ohl::platform::AtomicDirectoryPlan& plan) noexcept override {
    events.emplace_back("probe");
    observed_identity = std::string{plan.identity};
    observed_plan.clear();
    for (const auto& entry : plan.entries) {
      StoredEntry copy;
      copy.expected_size = entry.size_bytes;
      for (const auto component : entry.components) {
        copy.components.emplace_back(component);
      }
      observed_plan.push_back(std::move(copy));
    }
    if (probe_index < probes.size()) {
      return probes[probe_index++];
    }
    return {};
  }

  [[nodiscard]] ohl::platform::AtomicDirectoryTransactionResult
  create_transaction() noexcept override {
    events.emplace_back("create");
    if (failures.create_failure) {
      return {.transaction = nullptr,
              .error = AtomicDirectoryStoreError::resource_exhausted};
    }
    return {.transaction = std::make_unique<FakeTransaction>(*this)};
  }

  [[nodiscard]] bool should_fail(const std::size_t configured,
                                 std::size_t& counter) noexcept {
    ++counter;
    return configured != 0 && configured == counter;
  }

  FailureScript failures;
  std::vector<ohl::platform::AtomicDirectoryProbeResult> probes;
  std::size_t probe_index{0};
  std::vector<std::string> events;
  std::vector<OpenObservation> opened;
  std::vector<StoredEntry> observed_plan;
  std::vector<std::vector<std::byte>> expected_contents;
  std::vector<StoredEntry> published_entries;
  std::string published_identity;
  std::string observed_identity;
  std::string completion_identity;
  std::uint64_t completion_entries{0};
  std::uint64_t completion_bytes{0};
  std::uint64_t accepted_bytes{0};
  bool visible{false};
  std::size_t abort_calls{0};
  std::size_t publish_calls{0};
  std::size_t begin_calls{0};
  std::size_t open_calls{0};
  std::size_t write_calls{0};
  std::size_t seal_calls{0};
  std::size_t completion_calls{0};
  std::size_t sync_calls{0};
};

FakeSink::~FakeSink() {
  store_.events.emplace_back("close");
  open_ = false;
}

AtomicDirectoryStoreError FakeSink::write(
    const std::span<const std::byte> data) noexcept {
  store_.events.push_back("write:" + std::to_string(data.size()));
  if (store_.should_fail(store_.failures.write_call, store_.write_calls)) {
    return AtomicDirectoryStoreError::io_failure;
  }
  staged_data_.insert(staged_data_.end(), data.begin(), data.end());
  store_.accepted_bytes += static_cast<std::uint64_t>(data.size());
  return AtomicDirectoryStoreError::none;
}

AtomicDirectoryStoreError FakeTransaction::begin(
    const ohl::platform::AtomicDirectoryPlan& plan) noexcept {
  if (state_ != State::created) {
    return AtomicDirectoryStoreError::invalid_state;
  }
  store_.events.emplace_back("begin");
  state_ = State::staging;
  plan_identity_ = std::string{plan.identity};
  staged_entries_.clear();
  for (const auto& entry : plan.entries) {
    StoredEntry copy;
    copy.expected_size = entry.size_bytes;
    for (const auto component : entry.components) {
      copy.components.emplace_back(component);
    }
    staged_entries_.push_back(std::move(copy));
  }
  if (store_.should_fail(store_.failures.begin_call, store_.begin_calls)) {
    return AtomicDirectoryStoreError::io_failure;
  }
  return AtomicDirectoryStoreError::none;
}

ohl::platform::AtomicDirectoryOpenResult FakeTransaction::open_file(
    const std::span<const std::string_view> components,
    const std::uint64_t expected_size) noexcept {
  if (state_ != State::staging || open_ || completion_sealed_) {
    return {.sink = nullptr,
            .error = AtomicDirectoryStoreError::invalid_state};
  }
  std::string event{"open:"};
  OpenObservation observation;
  for (std::size_t index = 0; index < components.size(); ++index) {
    if (index != 0) {
      event.push_back('/');
    }
    event.append(components[index]);
    observation.components.emplace_back(components[index]);
  }
  event.push_back(':');
  event += std::to_string(expected_size);
  store_.events.push_back(std::move(event));
  observation.expected_size = expected_size;
  store_.opened.push_back(std::move(observation));
  if (sealed_entries_ >= staged_entries_.size() ||
      staged_entries_[sealed_entries_].components !=
          store_.opened.back().components ||
      staged_entries_[sealed_entries_].expected_size != expected_size) {
    return {.sink = nullptr,
            .error = AtomicDirectoryStoreError::unsafe_destination};
  }
  if (store_.should_fail(store_.failures.open_call, store_.open_calls)) {
    return {.sink = nullptr,
            .error = AtomicDirectoryStoreError::io_failure};
  }
  open_ = true;
  return {.sink = std::make_unique<FakeSink>(
              store_, open_, staged_entries_[sealed_entries_].data)};
}

AtomicDirectoryStoreError FakeTransaction::seal_file(
    std::unique_ptr<ohl::platform::AtomicDirectoryByteSink> sink) noexcept {
  if (state_ != State::staging || !open_ || sink == nullptr ||
      completion_sealed_) {
    return AtomicDirectoryStoreError::invalid_state;
  }
  store_.events.emplace_back("seal");
  sink.reset();
  const auto& staged = staged_entries_[sealed_entries_];
  if (!std::cmp_equal(staged.data.size(), staged.expected_size) ||
      (!store_.expected_contents.empty() &&
       (sealed_entries_ >= store_.expected_contents.size() ||
        staged.data != store_.expected_contents[sealed_entries_]))) {
    return AtomicDirectoryStoreError::unsafe_destination;
  }
  if (store_.should_fail(store_.failures.seal_call, store_.seal_calls)) {
    return AtomicDirectoryStoreError::io_failure;
  }
  ++sealed_entries_;
  return AtomicDirectoryStoreError::none;
}

AtomicDirectoryStoreError FakeTransaction::seal_completion(
    const ohl::platform::AtomicDirectoryCompletion& completion) noexcept {
  if (state_ != State::staging || open_ || completion_sealed_ ||
      sealed_entries_ != staged_entries_.size()) {
    return AtomicDirectoryStoreError::invalid_state;
  }
  store_.events.emplace_back("completion");
  store_.completion_identity = std::string{completion.identity};
  store_.completion_entries = completion.entry_count;
  store_.completion_bytes = completion.total_bytes;
  std::uint64_t expected_total = 0;
  for (const auto& entry : staged_entries_) {
    expected_total += entry.expected_size;
  }
  if (completion.identity != plan_identity_ ||
      !std::cmp_equal(completion.entry_count, staged_entries_.size()) ||
      completion.total_bytes != expected_total) {
    return AtomicDirectoryStoreError::unsafe_destination;
  }
  if (store_.should_fail(store_.failures.completion_call,
                         store_.completion_calls)) {
    return AtomicDirectoryStoreError::io_failure;
  }
  completion_sealed_ = true;
  if (store_.failures.after_completion) {
    store_.failures.after_completion();
  }
  return AtomicDirectoryStoreError::none;
}

ohl::platform::AtomicDirectoryPublishResult
FakeTransaction::publish_no_replace() noexcept {
  if (state_ != State::staging || open_ || !completion_sealed_) {
    return {.error = AtomicDirectoryStoreError::invalid_state};
  }
  store_.events.emplace_back("publish");
  ++store_.publish_calls;
  if (store_.failures.publish_call != 0 &&
      store_.failures.publish_call == store_.publish_calls) {
    return {.error = AtomicDirectoryStoreError::io_failure};
  }
  if (store_.failures.publish_lost_race) {
    return {.state =
                ohl::platform::AtomicDirectoryPublishState::destination_exists};
  }
  state_ = State::published;
  store_.visible = true;
  store_.published_identity = plan_identity_;
  store_.published_entries = staged_entries_;
  return {};
}

AtomicDirectoryStoreError FakeTransaction::sync_published_parent() noexcept {
  if (state_ != State::published) {
    return AtomicDirectoryStoreError::invalid_state;
  }
  store_.events.emplace_back("sync");
  if (store_.should_fail(store_.failures.sync_call, store_.sync_calls)) {
    return AtomicDirectoryStoreError::io_failure;
  }
  return AtomicDirectoryStoreError::none;
}

AtomicDirectoryStoreError FakeTransaction::abort() noexcept {
  store_.events.emplace_back("abort");
  ++store_.abort_calls;
  if (state_ == State::published || open_) {
    return AtomicDirectoryStoreError::invalid_state;
  }
  state_ = State::aborted;
  if (store_.failures.abort_call != 0 &&
      store_.failures.abort_call == store_.abort_calls) {
    return AtomicDirectoryStoreError::io_failure;
  }
  return AtomicDirectoryStoreError::none;
}

[[nodiscard]] ohl::media::PayloadStageRequest request_for(
    const std::vector<ohl::media::PlannedPayloadEntry>& entries,
    const std::uint64_t total, const std::string_view recipe = "recipe-v1",
    const ohl::media::PayloadImportLimits& limits = {}) {
  return {recipe, entries, total, limits};
}

[[nodiscard]] ohl::media::PayloadStageResult stage_payload(
    const ohl::media::PayloadStageRequest& request,
    ohl::media::PayloadSource& source,
    ohl::platform::AtomicDirectoryStore& store,
    const std::stop_token stop_token = {}) {
  return ohl::media::stage_payload(*default_media, request, source, store,
                                   stop_token);
}

[[nodiscard]] bool failed_at(const ohl::media::PayloadStageResult& result,
                             const ohl::media::PayloadStagePhase phase,
                             const ohl::media::PayloadStageError error) {
  return result.status == ohl::media::PayloadStageStatus::failed &&
         result.phase == phase && result.error == error;
}

[[nodiscard]] bool rejected_before_calls(
    const std::vector<ohl::media::PlannedPayloadEntry>& entries,
    const std::uint64_t total, const std::string_view recipe,
    const ohl::media::PayloadImportLimits& limits = {}) {
  ScriptedSource payload_source({});
  FakeStore store;
  const auto result = stage_payload(
      request_for(entries, total, recipe, limits), payload_source, store);
  return failed_at(result, ohl::media::PayloadStagePhase::validation,
                   ohl::media::PayloadStageError::invalid_request) &&
         store.events.empty() && payload_source.tokens.empty();
}

[[nodiscard]] std::string identity_for(
    const ohl::media::ValidatedMedia& media,
    const std::vector<ohl::media::PlannedPayloadEntry>& entries,
    const std::uint64_t total, const std::string_view recipe) {
  ScriptedSource payload_source({});
  FakeStore store;
  store.probes.push_back({AtomicDirectoryProbeState::matching,
                          AtomicDirectoryStoreError::none});
  return ohl::media::stage_payload(
             media, request_for(entries, total, recipe), payload_source, store)
      .identity;
}

[[nodiscard]] bool ends_with(const std::vector<std::string>& events,
                             const std::initializer_list<std::string_view> tail) {
  if (tail.size() > events.size()) {
    return false;
  }
  auto index = events.size() - tail.size();
  for (const auto expected : tail) {
    if (events[index++] != expected) {
      return false;
    }
  }
  return true;
}

}  // namespace

int main() {
  using ohl::media::PayloadPublicationState;
  using ohl::media::PayloadStageError;
  using ohl::media::PayloadStagePhase;
  using ohl::media::PayloadStageStatus;
  using ohl::media::PayloadStreamError;
  using ohl::media::PlannedPayloadEntry;

  ohl::media::test::SyntheticValidatedMedia media_fixture;
  default_media = &media_fixture.media();

  {
    const std::vector entries{
        PlannedPayloadEntry{1, "a/first", 2},
        PlannedPayloadEntry{2, "b", 0},
        PlannedPayloadEntry{3, "z/last", 1},
    };
    ScriptedSource source({{1, {bytes({1}), bytes({2})}, true},
                           {2, {}, true},
                           {3, {bytes({3})}, true}});
    FakeStore store;
    const auto result = stage_payload(
        request_for(entries, 3), source, store);
    const std::vector<std::string> expected_events{
        "probe",          "create",  "begin", "open:a/first:2",
        "write:1",        "write:1", "seal",  "close",
        "open:b:0",       "seal",    "close", "open:z/last:1",
        "write:1",        "seal",    "close", "completion",
        "publish",        "sync",
    };
    if (result.status != PayloadStageStatus::published_sync_complete ||
        result.phase != PayloadStagePhase::complete || !result.usable() ||
        result.error != PayloadStageError::none ||
        result.store_error != AtomicDirectoryStoreError::none ||
        result.cleanup_error != AtomicDirectoryStoreError::none ||
        result.publication !=
            PayloadPublicationState::published_sync_complete ||
        result.bytes_streamed != 3 || result.entries_streamed != 3 ||
        result.cleanup_attempted || store.events != expected_events ||
        !source.contract_ok || source.media_sources.size() != 3 ||
        source.stop_tokens != std::vector<std::stop_token>(3) ||
        source.sinks.size() != 3 ||
        source.tokens != std::vector<std::uint64_t>({1, 2, 3}) ||
        store.publish_calls != 1 || store.completion_entries != 3 ||
        store.completion_bytes != 3 ||
        store.completion_identity != result.identity ||
        store.published_identity != result.identity ||
        store.published_entries.size() != 3 ||
        store.published_entries[0].components !=
            std::vector<std::string>({"a", "first"}) ||
        store.published_entries[0].data != bytes({1, 2}) ||
        !store.published_entries[1].data.empty() ||
        store.published_entries[2].data != bytes({3})) {
      std::cerr << "successful deterministic staging order failed\n";
      return 1;
    }
  }

  {
    const std::string oversized_identity(
        ohl::media::kMaximumPayloadStageRecipeIdentityBytes + 1, 'x');
    const std::vector invalid_order{
        PlannedPayloadEntry{2, "b", 0}, PlannedPayloadEntry{1, "a", 0}};
    const std::vector one{PlannedPayloadEntry{1, "a", 1}};
    const std::vector unsafe{PlannedPayloadEntry{1, "../escape", 0}};
    const std::vector noncanonical{
        PlannedPayloadEntry{1, "dir\\file", 0}};
    const std::vector duplicate_token{
        PlannedPayloadEntry{1, "a", 0}, PlannedPayloadEntry{1, "b", 0}};
    const std::vector duplicate_path{
        PlannedPayloadEntry{1, "a", 0}, PlannedPayloadEntry{2, "a", 0}};
    const std::vector case_conflict{
        PlannedPayloadEntry{1, "a", 0}, PlannedPayloadEntry{2, "A", 0}};
    const std::vector prefix_conflict{
        PlannedPayloadEntry{1, "a", 0}, PlannedPayloadEntry{2, "a/b", 0}};

    const ohl::media::PayloadImportLimits count_limits{
        .maximum_entries = 1,
        .maximum_path_bytes = 100,
        .maximum_entry_bytes = 10,
        .maximum_total_bytes = 20,
    };
    const ohl::media::PayloadImportLimits path_limits{
        .maximum_entries = 3,
        .maximum_path_bytes = 3,
        .maximum_entry_bytes = 10,
        .maximum_total_bytes = 20,
    };
    const ohl::media::PayloadImportLimits entry_limits{
        .maximum_entries = 3,
        .maximum_path_bytes = 100,
        .maximum_entry_bytes = 1,
        .maximum_total_bytes = 3,
    };
    const ohl::media::PayloadImportLimits aggregate_limits{
        .maximum_entries = 3,
        .maximum_path_bytes = 100,
        .maximum_entry_bytes = 2,
        .maximum_total_bytes = 2,
    };
    const std::vector two_paths{
        PlannedPayloadEntry{1, "aa", 0}, PlannedPayloadEntry{2, "bb", 0}};
    const std::vector too_large{PlannedPayloadEntry{1, "a", 2}};
    const std::vector too_much{
        PlannedPayloadEntry{1, "a", 2}, PlannedPayloadEntry{2, "b", 1}};
    const std::vector overflowing{
        PlannedPayloadEntry{1, "a", std::numeric_limits<std::uint64_t>::max()},
        PlannedPayloadEntry{2, "b", 1}};
    const ohl::media::PayloadImportLimits overflow_limits{
        .maximum_entries = 3,
        .maximum_path_bytes = 100,
        .maximum_entry_bytes = std::numeric_limits<std::uint64_t>::max(),
        .maximum_total_bytes = std::numeric_limits<std::uint64_t>::max(),
    };

    const bool rejected =
        rejected_before_calls(one, 1, "") &&
        rejected_before_calls(one, 1, oversized_identity) &&
        rejected_before_calls(unsafe, 0, "recipe") &&
        rejected_before_calls(noncanonical, 0, "recipe") &&
        rejected_before_calls(duplicate_token, 0, "recipe") &&
        rejected_before_calls(duplicate_path, 0, "recipe") &&
        rejected_before_calls(case_conflict, 0, "recipe") &&
        rejected_before_calls(prefix_conflict, 0, "recipe") &&
        rejected_before_calls(invalid_order, 0, "recipe") &&
        rejected_before_calls(one, 2, "recipe") &&
        rejected_before_calls(duplicate_path, 0, "recipe", count_limits) &&
        rejected_before_calls(two_paths, 0, "recipe", path_limits) &&
        rejected_before_calls(too_large, 2, "recipe", entry_limits) &&
        rejected_before_calls(too_much, 3, "recipe", aggregate_limits) &&
        rejected_before_calls(overflowing, 0, "recipe", overflow_limits);
    if (!rejected) {
      std::cerr << "invalid request table reached the store or source\n";
      return 1;
    }
  }

  {
    const std::vector<PlannedPayloadEntry> entries;
    ScriptedSource source({});
    FakeStore store;
    const auto result = stage_payload(
        request_for(entries, 0), source, store);
    const std::vector<std::string> expected{
        "probe", "create", "begin", "completion", "publish", "sync"};
    if (result.status != PayloadStageStatus::published_sync_complete ||
        result.bytes_streamed != 0 || result.entries_streamed != 0 ||
        !source.tokens.empty() || store.events != expected) {
      std::cerr << "empty payload plan did not complete atomically\n";
      return 1;
    }
  }

  {
    const std::vector entries{PlannedPayloadEntry{1, "a", 1}};
    for (const bool create_failure : {false, true}) {
      ScriptedSource source({{1, {bytes({1})}, true}});
      FakeStore store;
      if (create_failure) {
        store.failures.create_failure = true;
      } else {
        store.probes.push_back({AtomicDirectoryProbeState::absent,
                                AtomicDirectoryStoreError::unsafe_destination});
      }
      const auto result = stage_payload(
          request_for(entries, 1), source, store);
      const auto expected_phase = create_failure
                                      ? PayloadStagePhase::create_transaction
                                      : PayloadStagePhase::probe;
      const auto expected_store_error =
          create_failure ? AtomicDirectoryStoreError::resource_exhausted
                         : AtomicDirectoryStoreError::unsafe_destination;
      if (!failed_at(result, expected_phase, PayloadStageError::store_failure) ||
          result.store_error != expected_store_error ||
          result.cleanup_attempted ||
          result.cleanup_error != AtomicDirectoryStoreError::none ||
          result.publication != PayloadPublicationState::not_published ||
          !source.tokens.empty() || store.abort_calls != 0 ||
          store.events.size() != (create_failure ? 2U : 1U)) {
        std::cerr << "pre-transaction store failure was mishandled\n";
        return 1;
      }
    }

    ScriptedSource unsupported_source({{1, {bytes({1})}, true}});
    FakeStore unsupported_store;
    unsupported_store.probes.push_back({
        AtomicDirectoryProbeState::absent,
        AtomicDirectoryStoreError::unsupported});
    const auto unsupported = stage_payload(
        request_for(entries, 1), unsupported_source, unsupported_store);
    if (!failed_at(unsupported, PayloadStagePhase::probe,
                   PayloadStageError::store_failure) ||
        unsupported.store_error != AtomicDirectoryStoreError::unsupported ||
        unsupported.cleanup_attempted || !unsupported_source.tokens.empty()) {
      std::cerr << "unsupported store capability was not preserved\n";
      return 1;
    }
  }

  {
    constexpr std::uint64_t large_size = (1ULL << 32U) + 17U;
    const std::vector entries{PlannedPayloadEntry{9, "large", large_size}};
    ScriptedSource source({});
    FakeStore store;
    store.failures.open_call = 1;
    const auto result = stage_payload(
        request_for(entries, large_size), source, store);
    if (!failed_at(result, PayloadStagePhase::open_file,
                   PayloadStageError::store_failure) ||
        store.opened.size() != 1 ||
        store.opened[0].expected_size != large_size ||
        store.observed_plan.size() != 1 ||
        store.observed_plan[0].expected_size != large_size ||
        result.store_error != AtomicDirectoryStoreError::io_failure ||
        result.cleanup_error != AtomicDirectoryStoreError::none ||
        result.publication != PayloadPublicationState::not_published ||
        !source.tokens.empty() || store.abort_calls != 1) {
      std::cerr << "large declared size was narrowed or consumed\n";
      return 1;
    }
  }

  {
    ohl::media::test::SyntheticValidatedMedia moved_fixture;
    auto valid_media = std::move(moved_fixture.media());
    ScriptedSource source({});
    FakeStore store;
    const std::vector<PlannedPayloadEntry> entries;
    const auto result = ohl::media::stage_payload(
        moved_fixture.media(), request_for(entries, 0), source, store);
    if (!failed_at(result, PayloadStagePhase::validation,
                   PayloadStageError::source_verification_failure) ||
        result.verification_error !=
            ohl::media::PayloadStageVerificationError::invalid_capability ||
        result.publication != PayloadPublicationState::not_published ||
        !store.events.empty() || !source.tokens.empty() || !valid_media.valid()) {
      std::cerr << "moved validated capability was not rejected early\n";
      return 1;
    }
  }

  {
    std::stop_source stop;
    (void)stop.request_stop();
    const std::vector entries{PlannedPayloadEntry{1, "cancel-before", 1}};
    ScriptedSource source({{1, {bytes({1})}, true}}, stop.get_token());
    FakeStore store;
    const auto result =
        stage_payload(request_for(entries, 1), source, store, stop.get_token());
    if (!failed_at(result, PayloadStagePhase::cancellation,
                   PayloadStageError::cancelled) ||
        result.identity.empty() || !store.events.empty() ||
        !source.tokens.empty() || result.cleanup_attempted ||
        result.publication != PayloadPublicationState::not_published) {
      std::cerr << "pre-cancellation reached store mutation\n";
      return 1;
    }
  }

  {
    const std::vector entries{PlannedPayloadEntry{1, "cancel-stream", 2}};
    for (const bool after_exact_stream : {false, true}) {
      std::stop_source stop;
      ScriptedSource source(
          {{1, after_exact_stream
                   ? std::vector<std::vector<std::byte>>{bytes({1, 2})}
                   : std::vector<std::vector<std::byte>>{bytes({1}), bytes({2})},
            true}},
          stop.get_token(), &stop, after_exact_stream ? 0U : 1U,
          after_exact_stream);
      FakeStore store;
      const auto result = stage_payload(request_for(entries, 2), source, store,
                                        stop.get_token());
      const auto expected_bytes = after_exact_stream ? 2U : 1U;
      if (!failed_at(result, PayloadStagePhase::stream_file,
                     PayloadStageError::cancelled) ||
          result.stream_error != PayloadStreamError::cancelled ||
          result.bytes_streamed != expected_bytes ||
          result.entries_streamed != 0 || !result.cleanup_attempted ||
          result.cleanup_error != AtomicDirectoryStoreError::none ||
          store.abort_calls != 1 || store.publish_calls != 0 ||
          !source.contract_ok ||
          source.stop_tokens != std::vector<std::stop_token>{stop.get_token()}) {
        std::cerr << "during/after-stream cancellation was mishandled\n";
        return 1;
      }
    }
  }

  {
    std::stop_source stop;
    const std::vector<PlannedPayloadEntry> entries;
    ScriptedSource source({});
    FakeStore store;
    store.failures.after_completion = [&stop]() {
      (void)stop.request_stop();
    };
    const auto result = stage_payload(request_for(entries, 0), source, store,
                                      stop.get_token());
    if (!failed_at(result, PayloadStagePhase::verify_source,
                   PayloadStageError::cancelled) ||
        result.verification_error !=
            ohl::media::PayloadStageVerificationError::cancelled ||
        result.entries_streamed != 0 || result.bytes_streamed != 0 ||
        !result.cleanup_attempted || store.abort_calls != 1 ||
        store.publish_calls != 0 ||
        !ends_with(store.events, {"completion", "abort"})) {
      std::cerr << "final pre-publication cancellation check failed\n";
      return 1;
    }
  }

  {
    for (const bool restore_write_time : {false, true}) {
      ohl::media::test::SyntheticValidatedMedia changed_fixture;
      default_media = &changed_fixture.media();
      const std::vector entries{PlannedPayloadEntry{1, "verified", 1}};
      ScriptedSource source({{1, {bytes({1})}, true}});
      FakeStore store;
      store.failures.abort_call = restore_write_time ? 1U : 0U;
      bool mutation_succeeded = false;
      store.failures.after_completion = [&]() {
        mutation_succeeded = changed_fixture.overwrite_byte(
            100U * ohl::media::test::kSyntheticSectorSize, std::byte{9},
            restore_write_time);
      };
      const auto result = stage_payload(request_for(entries, 1), source, store);
      const auto expected_verification =
          restore_write_time
              ? ohl::media::PayloadStageVerificationError::digest_mismatch
              : ohl::media::PayloadStageVerificationError::source_changed;
      const auto expected_cleanup =
          restore_write_time ? AtomicDirectoryStoreError::io_failure
                             : AtomicDirectoryStoreError::none;
      if (!mutation_succeeded ||
          !failed_at(result, PayloadStagePhase::verify_source,
                     PayloadStageError::source_verification_failure) ||
          result.verification_error != expected_verification ||
          result.publication != PayloadPublicationState::not_published ||
          result.entries_streamed != 1 || result.bytes_streamed != 1 ||
          !result.cleanup_attempted || result.cleanup_error != expected_cleanup ||
          store.abort_calls != 1 || store.publish_calls != 0 || store.visible ||
          !ends_with(store.events, {"completion", "abort"})) {
        std::cerr << "sealed source verification failure was mishandled\n";
        return 1;
      }
    }
    default_media = &media_fixture.media();
  }

  {
    const std::vector entries{PlannedPayloadEntry{1, "a", 1}};
    ScriptedSource source({{1, {bytes({1})}, true}});
    FakeStore store;
    store.failures.open_call = 1;
    store.failures.abort_call = 1;
    const auto result = stage_payload(
        request_for(entries, 1), source, store);
    if (!failed_at(result, PayloadStagePhase::open_file,
                   PayloadStageError::store_failure) ||
        result.store_error != AtomicDirectoryStoreError::io_failure ||
        result.cleanup_error != AtomicDirectoryStoreError::io_failure ||
        result.publication != PayloadPublicationState::not_published ||
        !result.cleanup_attempted || store.abort_calls != 1) {
      std::cerr << "primary store failure and abort failure were collapsed\n";
      return 1;
    }
  }

  {
    const std::vector entries{PlannedPayloadEntry{1, "a", 1}};
    for (const auto state : {AtomicDirectoryProbeState::matching,
                             AtomicDirectoryProbeState::conflict}) {
      ScriptedSource source({{1, {bytes({1})}, true}});
      FakeStore store;
      store.probes.push_back({state, AtomicDirectoryStoreError::none});
      const auto result = stage_payload(
          request_for(entries, 1), source, store);
      const auto expected = state == AtomicDirectoryProbeState::matching
                                ? PayloadStageStatus::cache_hit
                                : PayloadStageStatus::conflict;
      if (result.status != expected || result.error != PayloadStageError::none ||
          result.store_error != AtomicDirectoryStoreError::none ||
          result.cleanup_attempted ||
          result.publication != PayloadPublicationState::not_published ||
          store.events != std::vector<std::string>{"probe"} ||
          !source.tokens.empty()) {
        std::cerr << "existing destination did not short-circuit\n";
        return 1;
      }
    }
  }

  {
    const std::vector entries{PlannedPayloadEntry{1, "a", 2}};
    struct StreamCase {
      SourceScript source;
      std::size_t fail_write;
      PayloadStreamError error;
      std::uint64_t accepted;
    };
    const std::vector cases{
        StreamCase{{1, {bytes({1})}, true}, 0,
                   PayloadStreamError::underflow, 1},
        StreamCase{{1, {bytes({1, 2, 3})}, true}, 0,
                   PayloadStreamError::overflow, 0},
        StreamCase{{1, {bytes({1}), bytes({2, 3})}, true}, 0,
                   PayloadStreamError::overflow, 1},
        StreamCase{{1, {}, false}, 0, PayloadStreamError::source_failure, 0},
        StreamCase{{1, {bytes({1, 2})}, false}, 0,
                   PayloadStreamError::source_failure, 2},
        StreamCase{{1, {bytes({1}), bytes({2})}, true}, 2,
                   PayloadStreamError::destination_failure, 1},
    };
    for (const auto& test : cases) {
      ScriptedSource source({test.source});
      FakeStore store;
      store.failures.write_call = test.fail_write;
      const auto result = stage_payload(
          request_for(entries, 2), source, store);
      const auto expected_store_error =
          test.error == PayloadStreamError::destination_failure
              ? AtomicDirectoryStoreError::io_failure
              : AtomicDirectoryStoreError::none;
      if (!failed_at(result, PayloadStagePhase::stream_file,
                     PayloadStageError::stream_failure) ||
          result.stream_error != test.error || result.bytes_streamed != test.accepted ||
          result.store_error != expected_store_error ||
          result.cleanup_error != AtomicDirectoryStoreError::none ||
          result.publication != PayloadPublicationState::not_published ||
          result.entries_streamed != 0 || !result.failing_entry.has_value() ||
          *result.failing_entry != 0 || !result.cleanup_attempted ||
          store.abort_calls != 1 || !ends_with(store.events, {"close", "abort"}) ||
          store.publish_calls != 0) {
        std::cerr << "stream failure accounting or cleanup failed\n";
        return 1;
      }
    }
  }

  {
    const std::vector entries{PlannedPayloadEntry{1, "a", 1}};
    struct StoreCase {
      PayloadStagePhase phase;
      FailureScript failures;
      std::uint64_t bytes_streamed;
      std::uint64_t entries_streamed;
    };
    FailureScript begin;
    begin.begin_call = 1;
    FailureScript open;
    open.open_call = 1;
    FailureScript seal;
    seal.seal_call = 1;
    FailureScript completion;
    completion.completion_call = 1;
    FailureScript publish;
    publish.publish_call = 1;
    const std::vector cases{
        StoreCase{PayloadStagePhase::begin, begin, 0, 0},
        StoreCase{PayloadStagePhase::open_file, open, 0, 0},
        StoreCase{PayloadStagePhase::seal_file, seal, 1, 1},
        StoreCase{PayloadStagePhase::seal_completion, completion, 1, 1},
        StoreCase{PayloadStagePhase::publish, publish, 1, 1},
    };
    for (const auto& test : cases) {
      ScriptedSource source({{1, {bytes({1})}, true}});
      FakeStore store;
      store.failures = test.failures;
      const auto result = stage_payload(
          request_for(entries, 1), source, store);
      const auto expected_error = test.phase == PayloadStagePhase::publish
                                      ? PayloadStageError::publish_failure
                                      : PayloadStageError::store_failure;
      if (!failed_at(result, test.phase, expected_error) ||
          result.store_error != AtomicDirectoryStoreError::io_failure ||
          result.cleanup_error != AtomicDirectoryStoreError::none ||
          result.publication != PayloadPublicationState::not_published ||
          result.bytes_streamed != test.bytes_streamed ||
          result.entries_streamed != test.entries_streamed ||
          !result.cleanup_attempted || store.abort_calls != 1 ||
          !ends_with(store.events, {"abort"}) || store.visible ||
          (test.phase == PayloadStagePhase::seal_file &&
           !ends_with(store.events, {"seal", "close", "abort"}))) {
        std::cerr << "store phase failure did not abort exactly once\n";
        return 1;
      }
    }
  }

  {
    const std::vector entries{PlannedPayloadEntry{1, "a", 1}};
    ScriptedSource source({{1, {}, true}});
    FakeStore store;
    store.failures.abort_call = 1;
    const auto result = stage_payload(
        request_for(entries, 1), source, store);
    if (!failed_at(result, PayloadStagePhase::stream_file,
                   PayloadStageError::stream_failure) ||
        result.stream_error != PayloadStreamError::underflow ||
        result.cleanup_error != AtomicDirectoryStoreError::io_failure ||
        result.store_error != AtomicDirectoryStoreError::none ||
        result.publication != PayloadPublicationState::not_published ||
        result.cleanup_attempted != true ||
        store.abort_calls != 1) {
      std::cerr << "cleanup failure hid or replaced the primary failure\n";
      return 1;
    }
  }

  {
    const std::vector entries{PlannedPayloadEntry{1, "a", 1}};
    ScriptedSource source({{1, {bytes({1})}, true}});
    FakeStore store;
    store.failures.sync_call = 1;
    const auto result = stage_payload(
        request_for(entries, 1), source, store);
    if (result.status != PayloadStageStatus::published_sync_uncertain ||
        result.error != PayloadStageError::published_sync_failure ||
        result.publication !=
            PayloadPublicationState::published_sync_uncertain ||
        result.store_error != AtomicDirectoryStoreError::io_failure ||
        result.cleanup_attempted || store.abort_calls != 0 ||
        store.publish_calls != 1 || !store.visible ||
        !ends_with(store.events, {"publish", "sync"})) {
      std::cerr << "post-publication sync failure was mishandled\n";
      return 1;
    }
  }

  {
    const std::vector entries{PlannedPayloadEntry{1, "a", 1}};
    for (const auto winner : {AtomicDirectoryProbeState::matching,
                              AtomicDirectoryProbeState::conflict}) {
      ScriptedSource source({{1, {bytes({1})}, true}});
      FakeStore store;
      store.failures.publish_lost_race = true;
      store.probes = {{AtomicDirectoryProbeState::absent,
                       AtomicDirectoryStoreError::none},
                      {winner, AtomicDirectoryStoreError::none}};
      const auto result = stage_payload(
          request_for(entries, 1), source, store);
      const auto expected = winner == AtomicDirectoryProbeState::matching
                                ? PayloadStageStatus::cache_hit
                                : PayloadStageStatus::conflict;
      const auto expected_observation =
          winner == AtomicDirectoryProbeState::matching
              ? ohl::media::PayloadWinnerObservation::matching
              : ohl::media::PayloadWinnerObservation::conflict;
      if (result.status != expected || result.error != PayloadStageError::none ||
          result.winner_observation != expected_observation ||
          result.store_error != AtomicDirectoryStoreError::none ||
          result.cleanup_error != AtomicDirectoryStoreError::none ||
          result.phase != PayloadStagePhase::revalidate ||
          result.publication != PayloadPublicationState::not_published ||
          source.tokens != std::vector<std::uint64_t>{1} ||
          store.abort_calls != 1 || store.probe_index != 2 ||
          !ends_with(store.events, {"publish", "abort", "probe"})) {
        std::cerr << "lost publication race was not safely revalidated\n";
        return 1;
      }
    }
  }

  {
    const std::vector entries{PlannedPayloadEntry{1, "a", 1}};
    for (const auto winner : {
             ohl::platform::AtomicDirectoryProbeResult{
                 AtomicDirectoryProbeState::absent,
                 AtomicDirectoryStoreError::none},
             ohl::platform::AtomicDirectoryProbeResult{
                 AtomicDirectoryProbeState::absent,
                 AtomicDirectoryStoreError::io_failure}}) {
      ScriptedSource source({{1, {bytes({1})}, true}});
      FakeStore store;
      store.failures.publish_lost_race = true;
      store.probes = {{AtomicDirectoryProbeState::absent,
                       AtomicDirectoryStoreError::none},
                      winner};
      const auto result = stage_payload(
          request_for(entries, 1), source, store);
      const auto expected_observation =
          winner.error == AtomicDirectoryStoreError::none
              ? ohl::media::PayloadWinnerObservation::absent
              : ohl::media::PayloadWinnerObservation::probe_failed;
      if (!failed_at(result, PayloadStagePhase::revalidate,
                     PayloadStageError::revalidation_failure) ||
          result.winner_observation != expected_observation ||
          result.store_error != winner.error ||
          result.cleanup_error != AtomicDirectoryStoreError::none ||
          result.publication != PayloadPublicationState::not_published ||
          source.tokens != std::vector<std::uint64_t>{1} ||
          store.abort_calls != 1 || store.probe_index != 2) {
        std::cerr << "unresolved lost race did not fail revalidation\n";
        return 1;
      }
    }
  }

  {
    const std::vector entries{PlannedPayloadEntry{1, "a", 1}};
    for (const auto winner : {AtomicDirectoryProbeState::matching,
                              AtomicDirectoryProbeState::conflict}) {
      ScriptedSource source({{1, {bytes({1})}, true}});
      FakeStore store;
      store.failures.publish_lost_race = true;
      store.failures.abort_call = 1;
      store.probes = {{AtomicDirectoryProbeState::absent,
                       AtomicDirectoryStoreError::none},
                      {winner, AtomicDirectoryStoreError::none}};
      const auto result = stage_payload(
          request_for(entries, 1), source, store);
      const auto observation = winner == AtomicDirectoryProbeState::matching
                                   ? ohl::media::PayloadWinnerObservation::matching
                                   : ohl::media::PayloadWinnerObservation::conflict;
      if (result.status != PayloadStageStatus::cleanup_failed ||
          result.usable() || result.error != PayloadStageError::cleanup_failure ||
          result.phase != PayloadStagePhase::revalidate ||
          result.winner_observation != observation ||
          result.store_error != AtomicDirectoryStoreError::none ||
          result.cleanup_error != AtomicDirectoryStoreError::io_failure ||
          result.publication != PayloadPublicationState::not_published ||
          !result.cleanup_attempted || store.abort_calls != 1 ||
          store.probe_index != 2) {
        std::cerr << "lost-race cleanup failure was not preserved\n";
        return 1;
      }
    }
  }

  {
    const std::vector entries{PlannedPayloadEntry{1, "a", 1},
                              PlannedPayloadEntry{2, "b", 1},
                              PlannedPayloadEntry{3, "c", 1}};
    struct MiddleCase {
      PayloadStagePhase phase;
      PayloadStageError error;
      FailureScript failures;
      std::uint64_t bytes_streamed;
      std::uint64_t entries_streamed;
      std::vector<std::uint64_t> tokens;
    };
    FailureScript open;
    open.open_call = 2;
    FailureScript write;
    write.write_call = 2;
    FailureScript seal;
    seal.seal_call = 2;
    const std::vector cases{
        MiddleCase{PayloadStagePhase::open_file, PayloadStageError::store_failure,
                   open, 1, 1, {1}},
        MiddleCase{PayloadStagePhase::stream_file,
                   PayloadStageError::stream_failure, write, 1, 1, {1, 2}},
        MiddleCase{PayloadStagePhase::seal_file, PayloadStageError::store_failure,
                   seal, 2, 2, {1, 2}},
    };
    for (const auto& test : cases) {
      ScriptedSource source({{1, {bytes({1})}, true},
                             {2, {bytes({2})}, true},
                             {3, {bytes({3})}, true}});
      FakeStore store;
      store.failures = test.failures;
      const auto result = stage_payload(
          request_for(entries, 3), source, store);
      if (!failed_at(result, test.phase, test.error) ||
          result.store_error != AtomicDirectoryStoreError::io_failure ||
          result.cleanup_error != AtomicDirectoryStoreError::none ||
          result.publication != PayloadPublicationState::not_published ||
          result.bytes_streamed != test.bytes_streamed ||
          result.entries_streamed != test.entries_streamed ||
          !result.failing_entry.has_value() || *result.failing_entry != 1 ||
          source.tokens != test.tokens || store.opened.size() != 2 ||
          store.abort_calls != 1 || store.publish_calls != 0) {
        std::cerr << "middle-entry failure accounting failed\n";
        return 1;
      }
    }
  }

  {
    const std::vector base{PlannedPayloadEntry{1, "a", 0}};
    const std::vector different_token{PlannedPayloadEntry{
        std::numeric_limits<std::uint64_t>::max(), "a", 0}};
    const std::vector different_count{
        PlannedPayloadEntry{1, "a", 0}, PlannedPayloadEntry{2, "b", 0}};
    const std::vector different_path{PlannedPayloadEntry{1, "b", 0}};
    const std::vector different_size{PlannedPayloadEntry{1, "a", 1}};
    ohl::media::test::SyntheticValidatedMedia different_digest{
        ohl::media::test::kSyntheticMinimumSectorCount, std::byte{1}};
    ohl::media::test::SyntheticValidatedMedia different_size_media{
        ohl::media::test::kSyntheticMinimumSectorCount + 1};
    const auto identity = identity_for(media_fixture.media(), base, 0, "recipe");
    constexpr std::string_view known_identity =
        "ohl-payload-v2-sha256:"
        "4f24ebbb85eaa4d83cae493d862a9f497354e13189e7ec2f78c53e470e996841";
    const auto token_identity = identity_for(media_fixture.media(),
                                             different_token, 0, "recipe");
    const std::vector independently_varied{
        identity_for(different_digest.media(), base, 0, "recipe"),
        identity_for(different_size_media.media(), base, 0, "recipe"),
        identity_for(media_fixture.media(), base, 0, "recipe-2"),
        identity_for(media_fixture.media(), different_count, 0, "recipe"),
        identity_for(media_fixture.media(), different_path, 0, "recipe"),
        identity_for(media_fixture.media(), different_size, 1, "recipe"),
    };
    bool all_distinct = true;
    for (const auto& varied : independently_varied) {
      all_distinct = all_distinct && varied != identity;
    }
    const auto ambiguous_one =
        identity_for(media_fixture.media(), base, 0, "ab");
    const auto ambiguous_two =
        identity_for(media_fixture.media(), base, 0, "a");
    if (identity != known_identity || identity != token_identity ||
        !all_distinct ||
        ambiguous_one == ambiguous_two ||
        !identity.starts_with("ohl-payload-v2-sha256:") ||
        identity.size() != 86) {
      std::cerr << "canonical v2 identity field separation failed: "
                << identity << '\n';
      return 1;
    }
  }

  {
    const std::vector<std::string_view> components{"a"};
    const std::vector<std::string_view> wrong_components{"b"};
    const ohl::platform::AtomicDirectoryEntry entry{components, 1};
    const std::vector plan_entries{entry};
    const ohl::platform::AtomicDirectoryPlan plan{"identity", plan_entries};

    FakeStore wrong_path_store;
    auto wrong_path = wrong_path_store.create_transaction();
    if (wrong_path.transaction == nullptr ||
        wrong_path.transaction->begin(plan) !=
            AtomicDirectoryStoreError::none ||
        wrong_path.transaction->open_file(wrong_components, 1).error !=
            AtomicDirectoryStoreError::unsafe_destination ||
        wrong_path.transaction->abort() != AtomicDirectoryStoreError::none) {
      std::cerr << "fake store accepted the wrong component path\n";
      return 1;
    }

    FakeStore wrong_content_store;
    wrong_content_store.expected_contents = {bytes({9})};
    auto wrong_content = wrong_content_store.create_transaction();
    if (wrong_content.transaction == nullptr ||
        wrong_content.transaction->begin(plan) !=
            AtomicDirectoryStoreError::none) {
      std::cerr << "fake content-check setup failed\n";
      return 1;
    }
    auto wrong_sink = wrong_content.transaction->open_file(components, 1);
    const auto wrong_bytes = bytes({1});
    if (wrong_sink.sink == nullptr ||
        wrong_sink.sink->write(wrong_bytes) !=
            AtomicDirectoryStoreError::none ||
        wrong_content.transaction->seal_file(std::move(wrong_sink.sink)) !=
            AtomicDirectoryStoreError::unsafe_destination ||
        wrong_content.transaction->abort() !=
            AtomicDirectoryStoreError::none) {
      std::cerr << "fake store accepted wrong staged content\n";
      return 1;
    }

    FakeStore wrong_identity_store;
    auto wrong_identity = wrong_identity_store.create_transaction();
    const std::vector<ohl::platform::AtomicDirectoryEntry> empty_entries;
    const ohl::platform::AtomicDirectoryPlan empty_plan{"identity",
                                                        empty_entries};
    if (wrong_identity.transaction == nullptr ||
        wrong_identity.transaction->begin(empty_plan) !=
            AtomicDirectoryStoreError::none ||
        wrong_identity.transaction->seal_completion({"wrong", 0, 0}) !=
            AtomicDirectoryStoreError::unsafe_destination ||
        wrong_identity.transaction->abort() !=
            AtomicDirectoryStoreError::none) {
      std::cerr << "fake store accepted wrong completion identity\n";
      return 1;
    }
  }

  {
    FakeStore store;
    auto created = store.create_transaction();
    const std::vector<std::string_view> components{"a"};
    if (created.transaction == nullptr ||
        created.transaction->open_file(components, 0).error !=
            AtomicDirectoryStoreError::invalid_state ||
        created.transaction->publish_no_replace().error !=
            AtomicDirectoryStoreError::invalid_state ||
        created.transaction->abort() != AtomicDirectoryStoreError::none ||
        created.transaction->seal_completion({"identity", 0, 0}) !=
            AtomicDirectoryStoreError::invalid_state) {
      std::cerr << "fake store lifecycle misuse was not deterministic\n";
      return 1;
    }
  }

  {
    FakeStore store;
    auto created = store.create_transaction();
    const std::vector<std::string_view> components{"a"};
    const ohl::platform::AtomicDirectoryEntry entry{components, 0};
    const std::vector plan_entries{entry};
    const ohl::platform::AtomicDirectoryPlan plan{"identity", plan_entries};
    if (created.transaction == nullptr ||
        created.transaction->begin(plan) != AtomicDirectoryStoreError::none ||
        created.transaction->begin(plan) !=
            AtomicDirectoryStoreError::invalid_state) {
      std::cerr << "fake store setup failed\n";
      return 1;
    }
    auto first = created.transaction->open_file(components, 0);
    auto second = created.transaction->open_file(components, 0);
    if (first.sink == nullptr ||
        second.error != AtomicDirectoryStoreError::invalid_state) {
      std::cerr << "fake store allowed two open files\n";
      return 1;
    }
    first.sink.reset();
    if (created.transaction->abort() != AtomicDirectoryStoreError::none ||
        created.transaction->open_file(components, 0).error !=
            AtomicDirectoryStoreError::invalid_state) {
      std::cerr << "fake store allowed mutation after abort\n";
      return 1;
    }
  }

  {
    FakeStore store;
    auto created = store.create_transaction();
    const std::vector<ohl::platform::AtomicDirectoryEntry> no_entries;
    const ohl::platform::AtomicDirectoryPlan plan{"identity", no_entries};
    if (created.transaction == nullptr ||
        created.transaction->begin(plan) != AtomicDirectoryStoreError::none ||
        created.transaction->seal_completion({"identity", 0, 0}) !=
            AtomicDirectoryStoreError::none ||
        created.transaction->publish_no_replace().error !=
            AtomicDirectoryStoreError::none ||
        created.transaction->publish_no_replace().error !=
            AtomicDirectoryStoreError::invalid_state ||
        created.transaction->abort() != AtomicDirectoryStoreError::invalid_state) {
      std::cerr << "fake store allowed mutation after publication\n";
      return 1;
    }
  }

  return 0;
}
