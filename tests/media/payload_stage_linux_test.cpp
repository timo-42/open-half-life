#include "ohl/media/payload_stage.hpp"
#include "ohl/platform/atomic_directory_store.hpp"

#include "synthetic_media_test_support.hpp"

#include <fcntl.h>
#include <ftw.h>
#include <dirent.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>

#include <algorithm>
#include <array>
#include <cerrno>
#include <cstddef>
#include <cstdint>
#include <cstdlib>
#include <filesystem>
#include <cstdio>
#include <iostream>
#include <span>
#include <string>
#include <string_view>
#include <vector>

namespace {

int remove_item(const char* path, const struct stat*, int,
                struct FTW*) noexcept {
  return ::remove(path);
}

enum class RenameCapability { supported, unsupported, failed };

[[nodiscard]] RenameCapability rename_no_replace_capability(
    const std::string& root) {
  if (std::getenv("OHL_ATOMIC_STORE_TEST_FORCE_RENAME_UNSUPPORTED") !=
      nullptr) {
    return RenameCapability::unsupported;
  }
#if defined(SYS_renameat2)
  const auto source = root + "/.ohl-capability-source";
  const auto destination = root + "/.ohl-capability-destination";
  if (::mkdir(source.c_str(), 0700) != 0) {
    return RenameCapability::failed;
  }
  const auto result = static_cast<int>(
      ::syscall(SYS_renameat2, AT_FDCWD, source.c_str(), AT_FDCWD,
                destination.c_str(), RENAME_NOREPLACE));
  const auto rename_error = errno;
  if (::rmdir((result == 0 ? destination : source).c_str()) != 0) {
    return RenameCapability::failed;
  }
  if (result == 0) {
    return RenameCapability::supported;
  }
  return rename_error == ENOSYS || rename_error == EINVAL ||
                 rename_error == EOPNOTSUPP
             ? RenameCapability::unsupported
             : RenameCapability::failed;
#else
  (void)root;
  return RenameCapability::unsupported;
#endif
}

class Source final : public ohl::media::PayloadSource {
 public:
  explicit Source(const ohl::platform::MediaSource& expected_source) noexcept
      : expected_source_(&expected_source) {}

  [[nodiscard]] bool stream(
      const ohl::platform::MediaSource& media_source,
      const std::uint64_t token,
      const ohl::media::CancellationToken stop_token,
      ohl::media::PayloadByteSink& sink) noexcept override {
    ++calls;
    observed_source = &media_source;
    observed_token = token;
    observed_stop_token = stop_token;
    observed_sink = &sink;
    contract_ok = observed_source == expected_source_ &&
                  observed_stop_token == ohl::media::CancellationToken{} &&
                  observed_sink != nullptr;
    if (!contract_ok) {
      return false;
    }
    const std::array data{std::byte{'o'}, std::byte{'h'}, std::byte{'l'}};
    return sink.write(data);
  }

  std::size_t calls{0};
  std::uint64_t observed_token{0};
  const ohl::platform::MediaSource* observed_source{nullptr};
  ohl::media::CancellationToken observed_stop_token;
  const ohl::media::PayloadByteSink* observed_sink{nullptr};
  bool contract_ok{true};

 private:
  const ohl::platform::MediaSource* expected_source_{nullptr};
};

struct TransactionObservation {
  std::size_t publish_calls{0};
  std::size_t sync_calls{0};
  std::size_t abort_calls{0};
  bool committed_error_reconciled{false};
};

class WrappedTransaction final
    : public ohl::platform::AtomicDirectoryTransaction {
 public:
  WrappedTransaction(
      std::unique_ptr<ohl::platform::AtomicDirectoryTransaction> inner,
      const bool fail_sync, const bool model_reconciled_commit,
      TransactionObservation* observation) noexcept
      : inner_(std::move(inner)),
        fail_sync_(fail_sync),
        model_reconciled_commit_(model_reconciled_commit),
        observation_(observation) {}

  [[nodiscard]] ohl::platform::AtomicDirectoryStoreError begin(
      const ohl::platform::AtomicDirectoryPlan& plan) noexcept override {
    return inner_->begin(plan);
  }
  [[nodiscard]] ohl::platform::AtomicDirectoryOpenResult open_file(
      std::span<const std::string_view> components,
      std::uint64_t size) noexcept override {
    return inner_->open_file(components, size);
  }
  [[nodiscard]] ohl::platform::AtomicDirectoryStoreError seal_file(
      std::unique_ptr<ohl::platform::AtomicDirectoryByteSink> sink)
      noexcept override {
    return inner_->seal_file(std::move(sink));
  }
  [[nodiscard]] ohl::platform::AtomicDirectoryStoreError seal_completion(
      const ohl::platform::AtomicDirectoryCompletion& completion)
      noexcept override {
    return inner_->seal_completion(completion);
  }
  [[nodiscard]] ohl::platform::AtomicDirectoryPublishResult publish_no_replace()
      noexcept override {
    if (observation_ != nullptr) {
      ++observation_->publish_calls;
    }
    const auto publication = inner_->publish_no_replace();
    // The platform fault test injects an EIO after the native rename commits.
    // At this boundary the backend has reconciled that raw error to published.
    if (model_reconciled_commit_ && observation_ != nullptr &&
        publication.error == ohl::platform::AtomicDirectoryStoreError::none &&
        publication.state ==
            ohl::platform::AtomicDirectoryPublishState::published) {
      observation_->committed_error_reconciled = true;
    }
    return publication;
  }
  [[nodiscard]] ohl::platform::AtomicDirectoryStoreError
  sync_published_parent() noexcept override {
    if (observation_ != nullptr) {
      ++observation_->sync_calls;
    }
    return fail_sync_ ? ohl::platform::AtomicDirectoryStoreError::io_failure
                      : inner_->sync_published_parent();
  }
  [[nodiscard]] ohl::platform::AtomicDirectoryStoreError abort()
      noexcept override {
    if (observation_ != nullptr) {
      ++observation_->abort_calls;
    }
    return inner_->abort();
  }

 private:
  std::unique_ptr<ohl::platform::AtomicDirectoryTransaction> inner_;
  bool fail_sync_{false};
  bool model_reconciled_commit_{false};
  TransactionObservation* observation_{nullptr};
};

class WrappedStore final : public ohl::platform::AtomicDirectoryStore {
 public:
  WrappedStore(ohl::platform::AtomicDirectoryStore& inner,
               const bool absent_once, const bool fail_sync,
               const bool model_reconciled_commit = false,
               TransactionObservation* observation = nullptr) noexcept
      : inner_(inner),
        absent_once_(absent_once),
        fail_sync_(fail_sync),
        model_reconciled_commit_(model_reconciled_commit),
        observation_(observation) {}

  [[nodiscard]] ohl::platform::AtomicDirectoryProbeResult probe(
      const ohl::platform::AtomicDirectoryPlan& plan) noexcept override {
    if (absent_once_) {
      absent_once_ = false;
      return {};
    }
    return inner_.probe(plan);
  }
  [[nodiscard]] ohl::platform::AtomicDirectoryTransactionResult
  create_transaction() noexcept override {
    auto created = inner_.create_transaction();
    if (created.transaction == nullptr) {
      return created;
    }
    try {
      return {.transaction = std::make_unique<WrappedTransaction>(
                  std::move(created.transaction), fail_sync_,
                  model_reconciled_commit_, observation_)};
    } catch (...) {
      return {.transaction = nullptr,
              .error = ohl::platform::AtomicDirectoryStoreError::resource_exhausted};
    }
  }

 private:
  ohl::platform::AtomicDirectoryStore& inner_;
  bool absent_once_{false};
  bool fail_sync_{false};
  bool model_reconciled_commit_{false};
  TransactionObservation* observation_{nullptr};
};

[[nodiscard]] std::string final_name(const std::string_view identity) {
  constexpr char hex[] = "0123456789abcdef";
  std::string result{"ohl-tree-"};
  for (const auto character : identity) {
    const auto value = static_cast<unsigned char>(character);
    result.push_back(hex[value >> 4U]);
    result.push_back(hex[value & 0x0fU]);
  }
  return result;
}

}  // namespace

int main() {
  const auto* configured_parent =
      std::getenv("OHL_ATOMIC_STORE_TEST_PARENT");
  std::string pattern = configured_parent == nullptr
                            ? "/dev/shm"
                            : configured_parent;
  pattern += "/ohl-stage-native-XXXXXX";
  std::vector<char> mutable_pattern(pattern.begin(), pattern.end());
  mutable_pattern.push_back('\0');
  auto* created = ::mkdtemp(mutable_pattern.data());
  if (created == nullptr) {
#if OHL_REQUIRE_LINUX_ATOMIC_STORE_CAPABILITIES
    std::cerr << "FAIL: required writable native filesystem fixture unavailable\n";
    return 1;
#else
    std::cerr << "SKIP: writable native filesystem fixture unavailable\n";
    return 77;
#endif
  }
  const std::string root{created};
  const auto cleanup = [&root]() {
    (void)::nftw(root.c_str(), remove_item, 32, FTW_DEPTH | FTW_PHYS);
  };
  const auto rename_capability = rename_no_replace_capability(root);
  if (rename_capability == RenameCapability::unsupported) {
    cleanup();
#if OHL_REQUIRE_LINUX_ATOMIC_STORE_CAPABILITIES
    std::cerr << "FAIL: required renameat2 RENAME_NOREPLACE unavailable\n";
    return 1;
#else
    std::cerr << "SKIP: renameat2 RENAME_NOREPLACE unavailable\n";
    return 77;
#endif
  }
  if (rename_capability == RenameCapability::failed) {
    cleanup();
    return 1;
  }
  auto opened = ohl::platform::open_atomic_directory_store(
      std::filesystem::path{root});
  if (opened.error ==
      ohl::platform::AtomicDirectoryStoreError::unsupported) {
    cleanup();
#if OHL_REQUIRE_LINUX_ATOMIC_STORE_CAPABILITIES
    std::cerr << "FAIL: required Linux atomic-store filesystem capability unavailable\n";
    return 1;
#else
    std::cerr << "SKIP: Linux atomic-store filesystem capability unavailable\n";
    return 77;
#endif
  }
  if (opened.error != ohl::platform::AtomicDirectoryStoreError::none ||
      opened.store == nullptr) {
    cleanup();
    return 1;
  }
  const std::vector entries{
      ohl::media::PlannedPayloadEntry{
          42, "ProjectFixture/AmberPayload.dat", 3}};
  ohl::media::test::SyntheticValidatedMedia media_fixture;
  const auto& media = media_fixture.media();
  const auto& media_source = *media.source();
  const ohl::media::PayloadStageRequest request{
      "recipe", entries, 3, {}};
  Source first_source{media_source};
  const auto first =
      ohl::media::stage_payload(media, request, first_source, *opened.store);
  if (first.status != ohl::media::PayloadStageStatus::published_sync_complete ||
      first_source.calls != 1 || first_source.observed_token != 42 ||
      !first_source.contract_ok ||
      first_source.observed_source != &media_source ||
      first_source.observed_sink == nullptr) {
    cleanup();
    return 1;
  }
  WrappedStore lost_race_store{*opened.store, true, false};
  Source lost_source{media_source};
  const auto lost =
      ohl::media::stage_payload(media, request, lost_source, lost_race_store);
  if (lost.status != ohl::media::PayloadStageStatus::cache_hit ||
      lost.winner_observation !=
          ohl::media::PayloadWinnerObservation::matching ||
      lost_source.calls != 1 || !lost.cleanup_attempted) {
    cleanup();
    return 1;
  }
  Source hit_source{media_source};
  const auto hit =
      ohl::media::stage_payload(media, request, hit_source, *opened.store);
  if (hit.status != ohl::media::PayloadStageStatus::cache_hit ||
      hit_source.calls != 0) {
    cleanup();
    return 1;
  }
  const ohl::media::PayloadStageRequest sync_request{
      "sync-recipe", entries, 3, {}};
  WrappedStore sync_store{*opened.store, false, true};
  Source sync_source{media_source};
  const auto uncertain =
      ohl::media::stage_payload(media, sync_request, sync_source, sync_store);
  if (uncertain.status !=
          ohl::media::PayloadStageStatus::published_sync_uncertain ||
      uncertain.publication !=
          ohl::media::PayloadPublicationState::published_sync_uncertain ||
      uncertain.cleanup_attempted) {
    cleanup();
    return 1;
  }
  const ohl::media::PayloadStageRequest reconciled_request{
      "committed-error-reconciled", entries, 3, {}};
  TransactionObservation reconciled_observation;
  WrappedStore reconciled_store{*opened.store, false, false, true,
                                &reconciled_observation};
  Source reconciled_source{media_source};
  const auto reconciled = ohl::media::stage_payload(
      media, reconciled_request, reconciled_source, reconciled_store);
  if (reconciled.status !=
          ohl::media::PayloadStageStatus::published_sync_complete ||
      reconciled.publication !=
          ohl::media::PayloadPublicationState::published_sync_complete ||
      reconciled.cleanup_attempted || reconciled_source.calls != 1 ||
      reconciled_observation.publish_calls != 1 ||
      reconciled_observation.sync_calls != 1 ||
      reconciled_observation.abort_calls != 0 ||
      !reconciled_observation.committed_error_reconciled) {
    cleanup();
    return 1;
  }
  const auto final = final_name(first.identity);
  const auto extra_path = root + "/" + final + "/files/extra";
  const auto extra =
      ::open(extra_path.c_str(), O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, 0600);
  if (extra < 0 || ::close(extra) != 0) {
    cleanup();
    return 1;
  }
  Source conflict_source{media_source};
  const auto conflict =
      ohl::media::stage_payload(media, request, conflict_source, *opened.store);
  const auto result = conflict.status == ohl::media::PayloadStageStatus::conflict &&
                              conflict_source.calls == 0
                          ? 0
                          : 1;
  cleanup();
  return result;
}
