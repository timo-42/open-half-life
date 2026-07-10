#include "atomic_directory_store_internal.hpp"
#include "ohl/platform/atomic_directory_store.hpp"

#include <fcntl.h>
#include <ftw.h>
#include <dirent.h>
#include <poll.h>
#include <signal.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

#include <array>
#include <algorithm>
#include <cerrno>
#include <cstddef>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <cstdio>
#include <filesystem>
#include <iostream>
#include <map>
#include <memory>
#include <new>
#include <span>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace {

using ohl::platform::AtomicDirectoryProbeState;
using ohl::platform::AtomicDirectoryPublishState;
using ohl::platform::AtomicDirectoryStore;
using ohl::platform::AtomicDirectoryStoreError;
using ohl::platform::AtomicDirectoryTransaction;

constexpr long kExtFamilySuperMagic = 0xEF53;

int remove_item(const char* path, const struct stat*, int,
                struct FTW*) noexcept {
  return ::remove(path);
}

class TemporaryRoot final {
 public:
  TemporaryRoot() {
    const auto* configured_parent =
        std::getenv("OHL_ATOMIC_STORE_TEST_PARENT");
    std::string pattern = configured_parent == nullptr
                              ? "/dev/shm"
                              : configured_parent;
    pattern += "/ohl-atomic-XXXXXX";
    std::vector<char> mutable_pattern(pattern.begin(), pattern.end());
    mutable_pattern.push_back('\0');
    if (auto* created = ::mkdtemp(mutable_pattern.data()); created != nullptr) {
      path_ = created;
    }
  }
  ~TemporaryRoot() {
    if (!path_.empty()) {
      (void)::nftw(path_.c_str(), remove_item, 32, FTW_DEPTH | FTW_PHYS);
    }
  }

  TemporaryRoot(const TemporaryRoot&) = delete;
  TemporaryRoot& operator=(const TemporaryRoot&) = delete;
  [[nodiscard]] const std::string& path() const noexcept { return path_; }
  [[nodiscard]] bool valid() const noexcept { return !path_.empty(); }

 private:
  std::string path_;
};

class RemoveFileOnExit final {
 public:
  explicit RemoveFileOnExit(std::string path) : path_(std::move(path)) {}
  ~RemoveFileOnExit() {
    if (!path_.empty()) {
      (void)::unlink(path_.c_str());
    }
  }

  RemoveFileOnExit(const RemoveFileOnExit&) = delete;
  RemoveFileOnExit& operator=(const RemoveFileOnExit&) = delete;
  [[nodiscard]] const std::string& path() const noexcept { return path_; }

 private:
  std::string path_;
};

struct PlanOwner {
  PlanOwner(std::string identity_value,
            std::vector<std::vector<std::string>> component_values,
            std::vector<std::uint64_t> sizes)
      : identity(std::move(identity_value)),
        components(std::move(component_values)) {
    views.reserve(components.size());
    entries.reserve(components.size());
    for (auto& path : components) {
      auto& path_views = views.emplace_back();
      path_views.reserve(path.size());
      for (const auto& component : path) {
        path_views.emplace_back(component);
      }
    }
    for (std::size_t index = 0; index < views.size(); ++index) {
      entries.push_back({views[index], sizes[index]});
      total += sizes[index];
    }
  }

  [[nodiscard]] ohl::platform::AtomicDirectoryPlan plan() const noexcept {
    return {identity, entries};
  }
  [[nodiscard]] ohl::platform::AtomicDirectoryCompletion completion()
      const noexcept {
    return {identity, static_cast<std::uint64_t>(entries.size()), total};
  }

  std::string identity;
  std::vector<std::vector<std::string>> components;
  std::vector<std::vector<std::string_view>> views;
  std::vector<ohl::platform::AtomicDirectoryEntry> entries;
  std::uint64_t total{0};
};

[[nodiscard]] std::string hex_identity(const std::string_view identity) {
  constexpr char hex[] = "0123456789abcdef";
  std::string result{"ohl-tree-"};
  for (const auto character : identity) {
    const auto value = static_cast<unsigned char>(character);
    result.push_back(hex[value >> 4U]);
    result.push_back(hex[value & 0x0fU]);
  }
  return result;
}

[[nodiscard]] std::vector<std::byte> as_bytes(
    const std::string_view value) {
  const auto input = std::as_bytes(std::span{value.data(), value.size()});
  return {input.begin(), input.end()};
}

void append_big_u64(std::vector<std::byte>& output,
                    const std::uint64_t value) {
  for (std::size_t index = 0; index < 8; ++index) {
    output.push_back(static_cast<std::byte>(
        (value >> ((7U - index) * 8U)) & 0xffU));
  }
}

[[nodiscard]] std::vector<std::byte> expected_marker(const PlanOwner& plan) {
  auto result = as_bytes("OHLASTG1");
  append_big_u64(result, static_cast<std::uint64_t>(plan.identity.size()));
  append_big_u64(result, static_cast<std::uint64_t>(plan.entries.size()));
  append_big_u64(result, plan.total);
  const auto identity = as_bytes(plan.identity);
  result.insert(result.end(), identity.begin(), identity.end());
  return result;
}

[[nodiscard]] int capability_result(
    const AtomicDirectoryStoreError error, const std::string_view context) {
  if (error != AtomicDirectoryStoreError::unsupported) {
    return 1;
  }
#if OHL_REQUIRE_LINUX_ATOMIC_STORE_CAPABILITIES
  std::cerr << "FAIL: required Linux atomic-store capability unavailable: "
            << context << '\n';
  return 1;
#else
  std::cerr << "SKIP: Linux atomic-store capability unavailable: " << context
            << '\n';
  return 77;
#endif
}

[[nodiscard]] ohl::platform::AtomicDirectoryStoreOpenResult open_store(
    const TemporaryRoot& root) {
  return ohl::platform::open_atomic_directory_store(
      std::filesystem::path{root.path()});
}

struct ReadyTransaction {
  std::unique_ptr<AtomicDirectoryTransaction> transaction;
  AtomicDirectoryStoreError error{AtomicDirectoryStoreError::none};
};

[[nodiscard]] std::string find_stage(const std::string& root);

[[nodiscard]] ReadyTransaction make_ready(
    AtomicDirectoryStore& store, const PlanOwner& plan,
    const std::vector<std::string>& contents) {
  auto created = store.create_transaction();
  if (created.error != AtomicDirectoryStoreError::none ||
      created.transaction == nullptr) {
    return {nullptr, created.error};
  }
  auto& transaction = *created.transaction;
  auto error = transaction.begin(plan.plan());
  for (std::size_t index = 0;
       error == AtomicDirectoryStoreError::none && index < contents.size();
       ++index) {
    auto opened = transaction.open_file(plan.views[index],
                                        plan.entries[index].size_bytes);
    if (opened.error != AtomicDirectoryStoreError::none ||
        opened.sink == nullptr) {
      error = opened.error;
      break;
    }
    const auto data = as_bytes(contents[index]);
    error = opened.sink->write(data);
    if (error == AtomicDirectoryStoreError::none) {
      error = transaction.seal_file(std::move(opened.sink));
    }
  }
  if (error == AtomicDirectoryStoreError::none) {
    error = transaction.seal_completion(plan.completion());
  }
  return {std::move(created.transaction), error};
}

[[nodiscard]] bool probe_is(AtomicDirectoryStore& store, const PlanOwner& plan,
                            const AtomicDirectoryProbeState state) {
  const auto result = store.probe(plan.plan());
  return result.error == AtomicDirectoryStoreError::none &&
         result.state == state;
}

[[nodiscard]] int publication_capability_preflight() {
  if (std::getenv("OHL_ATOMIC_STORE_TEST_FORCE_RENAME_UNSUPPORTED") !=
      nullptr) {
    return capability_result(AtomicDirectoryStoreError::unsupported,
                             "forced renameat2 preflight result");
  }
  TemporaryRoot root;
  if (!root.valid()) {
    return capability_result(AtomicDirectoryStoreError::unsupported,
                             "writable test parent");
  }
  auto opened = open_store(root);
  if (opened.error != AtomicDirectoryStoreError::none ||
      opened.store == nullptr) {
    return opened.error == AtomicDirectoryStoreError::unsupported
               ? capability_result(opened.error, "allow-listed filesystem")
               : 1;
  }
  PlanOwner plan{"capability-preflight", {}, {}};
  auto ready = make_ready(*opened.store, plan, {});
  if (ready.error != AtomicDirectoryStoreError::none ||
      ready.transaction == nullptr) {
    return 1;
  }
  const auto publication = ready.transaction->publish_no_replace();
  if (publication.error == AtomicDirectoryStoreError::unsupported) {
    (void)ready.transaction->abort();
    return capability_result(publication.error,
                             "renameat2 RENAME_NOREPLACE");
  }
  return publication.error == AtomicDirectoryStoreError::none &&
                 publication.state == AtomicDirectoryPublishState::published &&
                 ready.transaction->sync_published_parent() ==
                     AtomicDirectoryStoreError::none &&
                 probe_is(*opened.store, plan,
                          AtomicDirectoryProbeState::matching) &&
                 find_stage(root.path()).empty()
             ? 0
             : 1;
}

[[nodiscard]] std::string find_stage(const std::string& root) {
  DIR* directory = ::opendir(root.c_str());
  if (directory == nullptr) {
    return {};
  }
  std::string result;
  while (auto* item = ::readdir(directory)) {
    const std::string_view name{item->d_name};
    if (name.starts_with(".ohl-stage-")) {
      result = std::string{name};
      break;
    }
  }
  (void)::closedir(directory);
  return result;
}

[[nodiscard]] bool exists(const std::string& path) {
  struct stat status {};
  return ::lstat(path.c_str(), &status) == 0;
}

[[nodiscard]] bool write_file(const std::string& path,
                              const std::string_view bytes) {
  const auto descriptor =
      ::open(path.c_str(), O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC, 0600);
  if (descriptor < 0) {
    return false;
  }
  const auto count = ::write(descriptor, bytes.data(), bytes.size());
  const auto close_result = ::close(descriptor);
  return count == static_cast<ssize_t>(bytes.size()) && close_result == 0;
}

[[nodiscard]] bool write_bytes(const std::string& path,
                               const std::span<const std::byte> bytes) {
  const auto descriptor =
      ::open(path.c_str(), O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC, 0600);
  if (descriptor < 0) {
    return false;
  }
  std::size_t offset = 0;
  while (offset < bytes.size()) {
    const auto count =
        ::write(descriptor, bytes.data() + offset, bytes.size() - offset);
    if (count < 0 && errno == EINTR) {
      continue;
    }
    if (count <= 0) {
      (void)::close(descriptor);
      return false;
    }
    offset += static_cast<std::size_t>(count);
  }
  return ::close(descriptor) == 0;
}

[[nodiscard]] std::vector<std::byte> read_bytes(const std::string& path) {
  const auto descriptor = ::open(path.c_str(), O_RDONLY | O_CLOEXEC);
  if (descriptor < 0) {
    return {};
  }
  std::vector<std::byte> result;
  std::array<std::byte, 256> buffer{};
  while (true) {
    const auto count = ::read(descriptor, buffer.data(), buffer.size());
    if (count < 0 && errno == EINTR) {
      continue;
    }
    if (count < 0) {
      result.clear();
    } else if (count > 0) {
      result.insert(result.end(), buffer.begin(), buffer.begin() + count);
      continue;
    }
    break;
  }
  return ::close(descriptor) == 0 ? result : std::vector<std::byte>{};
}

[[nodiscard]] bool read_full(const int descriptor,
                             const std::span<char> bytes) {
  std::size_t offset = 0;
  while (offset < bytes.size()) {
    struct pollfd pending {
      .fd = descriptor, .events = POLLIN, .revents = 0
    };
    int poll_result = 0;
    do {
      poll_result = ::poll(&pending, 1, 5'000);
    } while (poll_result < 0 && errno == EINTR);
    if (poll_result != 1 || (pending.revents & POLLIN) == 0) {
      return false;
    }
    const auto count =
        ::read(descriptor, bytes.data() + offset, bytes.size() - offset);
    if (count < 0 && errno == EINTR) {
      continue;
    }
    if (count <= 0) {
      return false;
    }
    offset += static_cast<std::size_t>(count);
  }
  return true;
}

[[nodiscard]] int test_root_security() {
  const auto relative = ohl::platform::open_atomic_directory_store("relative");
  if (relative.error != AtomicDirectoryStoreError::unsafe_destination) {
    return 1;
  }
  TemporaryRoot root;
  if (!root.valid()) {
    return 1;
  }
  if (::chmod(root.path().c_str(), 0770) != 0) {
    return 1;
  }
  if (open_store(root).error != AtomicDirectoryStoreError::unsafe_destination ||
      ::chmod(root.path().c_str(), 0700) != 0) {
    return 1;
  }
  const auto link = root.path() + "-link";
  if (::symlink(root.path().c_str(), link.c_str()) != 0) {
    return 1;
  }
  const auto linked = ohl::platform::open_atomic_directory_store(link);
  (void)::unlink(link.c_str());
  if (linked.error != AtomicDirectoryStoreError::unsafe_destination) {
    return 1;
  }
  const auto wrong_type = root.path() + "-file";
  if (!write_file(wrong_type, "x") ||
      ohl::platform::open_atomic_directory_store(wrong_type).error !=
          AtomicDirectoryStoreError::unsafe_destination ||
      ::unlink(wrong_type.c_str()) != 0 ||
      ::mkdir((root.path() + "/child").c_str(), 0700) != 0 ||
      ::symlink(root.path().c_str(), link.c_str()) != 0 ||
      ohl::platform::open_atomic_directory_store(link + "/child").error !=
          AtomicDirectoryStoreError::unsafe_destination ||
      ::unlink(link.c_str()) != 0 ||
      ohl::platform::open_atomic_directory_store(root.path() + "/../" +
                                                 root.path().substr(
                                                     root.path().find_last_of('/') +
                                                     1))
              .error != AtomicDirectoryStoreError::unsafe_destination) {
    return 1;
  }
  auto opened = open_store(root);
  if (opened.error == AtomicDirectoryStoreError::unsupported) {
    return 1;
  }
  if (opened.error != AtomicDirectoryStoreError::none || opened.store == nullptr) {
    return 1;
  }
  const std::string oversized_identity(
      ohl::platform::kMaximumAtomicDirectoryStoreIdentityBytes + 1, 'x');
  PlanOwner invalid_plan{oversized_identity, {{"f"}}, {0}};
  auto invalid_transaction = opened.store->create_transaction();
  if (invalid_transaction.transaction == nullptr ||
      invalid_transaction.transaction->begin(invalid_plan.plan()) !=
          AtomicDirectoryStoreError::invalid_state ||
      invalid_transaction.transaction->abort() !=
          AtomicDirectoryStoreError::none) {
    return 1;
  }
  const auto moved = root.path() + "-moved";
  if (::rename(root.path().c_str(), moved.c_str()) != 0 ||
      ::mkdir(root.path().c_str(), 0700) != 0) {
    return 1;
  }
  PlanOwner plan{"pin", {{"a"}}, {1}};
  auto ready = make_ready(*opened.store, plan, {"x"});
  if (ready.error != AtomicDirectoryStoreError::none ||
      ready.transaction->publish_no_replace().error !=
          AtomicDirectoryStoreError::none ||
      !exists(moved + "/" + hex_identity(plan.identity)) ||
      exists(root.path() + "/" + hex_identity(plan.identity))) {
    return 1;
  }
  (void)::rmdir(root.path().c_str());
  (void)::rename(moved.c_str(), root.path().c_str());
  return 0;
}

[[nodiscard]] int test_component_io() {
  TemporaryRoot root;
  if (!root.valid()) {
    return 1;
  }
  auto opened = open_store(root);
  if (opened.error == AtomicDirectoryStoreError::unsupported) {
    return 1;
  }
  PlanOwner plan{"component", {{"a", "b"}, {"zero"}}, {4, 0}};
  auto created = opened.store->create_transaction();
  if (created.transaction == nullptr ||
      created.transaction->begin(plan.plan()) != AtomicDirectoryStoreError::none) {
    std::cerr << "component: begin\n";
    return 1;
  }
  auto file = created.transaction->open_file(plan.views[0], 4);
  const auto first = as_bytes("ab");
  const auto second = as_bytes("cd");
  if (file.sink == nullptr ||
      file.sink->write(first) != AtomicDirectoryStoreError::none ||
      file.sink->write(second) != AtomicDirectoryStoreError::none ||
      file.sink->write(as_bytes("x")) !=
          AtomicDirectoryStoreError::invalid_state ||
      created.transaction->seal_file(std::move(file.sink)) !=
          AtomicDirectoryStoreError::invalid_state) {
    std::cerr << "component: overflow\n";
    return 1;
  }
  file.sink.reset();
  if (created.transaction->abort() != AtomicDirectoryStoreError::none) {
    std::cerr << "component: overflow abort\n";
    return 1;
  }

  auto underflow = opened.store->create_transaction();
  if (underflow.transaction->begin(plan.plan()) !=
      AtomicDirectoryStoreError::none) {
    std::cerr << "component: underflow begin\n";
    return 1;
  }
  auto short_file = underflow.transaction->open_file(plan.views[0], 4);
  if (short_file.sink == nullptr ||
      short_file.sink->write(first) != AtomicDirectoryStoreError::none) {
    std::cerr << "component: underflow\n";
    return 1;
  }
  const auto short_seal =
      underflow.transaction->seal_file(std::move(short_file.sink));
  const auto short_abort = underflow.transaction->abort();
  if (short_seal != AtomicDirectoryStoreError::invalid_state ||
      short_abort != AtomicDirectoryStoreError::none) {
    std::cerr << "component: underflow seal="
              << static_cast<int>(short_seal)
              << " abort=" << static_cast<int>(short_abort) << '\n';
    return 1;
  }

  auto external = opened.store->create_transaction();
  if (external.transaction->begin(plan.plan()) !=
      AtomicDirectoryStoreError::none) {
    std::cerr << "component: external begin\n";
    return 1;
  }
  const auto external_stage = find_stage(root.path());
  auto external_file = external.transaction->open_file(plan.views[0], 4);
  if (external_stage.empty() || external_file.sink == nullptr ||
      external_file.sink->write(as_bytes("abcd")) !=
          AtomicDirectoryStoreError::none ||
      ::truncate((root.path() + "/" + external_stage + "/files/a/b").c_str(),
                 2) != 0) {
    std::cerr << "component: external\n";
    return 1;
  }
  const auto external_seal =
      external.transaction->seal_file(std::move(external_file.sink));
  const auto external_abort = external.transaction->abort();
  if (external_seal != AtomicDirectoryStoreError::unsafe_destination ||
      external_abort != AtomicDirectoryStoreError::none) {
    std::cerr << "component: external seal="
              << static_cast<int>(external_seal)
              << " abort=" << static_cast<int>(external_abort) << '\n';
    return 1;
  }

  auto linked = opened.store->create_transaction();
  if (linked.transaction->begin(plan.plan()) != AtomicDirectoryStoreError::none) {
    std::cerr << "component: link begin\n";
    return 1;
  }
  const auto linked_stage = find_stage(root.path());
  const auto sentinel = root.path() + "-outside-sentinel";
  if (!write_file(sentinel, "safe")) {
    return 1;
  }
  if (linked_stage.empty() ||
      ::symlink(sentinel.c_str(),
                (root.path() + "/" + linked_stage + "/files/a").c_str()) != 0 ||
      linked.transaction->open_file(plan.views[0], 4).error !=
          AtomicDirectoryStoreError::unsafe_destination ||
      linked.transaction->abort() != AtomicDirectoryStoreError::none) {
    std::cerr << "component: link\n";
    return 1;
  }
  struct stat sentinel_status {};
  if (::stat(sentinel.c_str(), &sentinel_status) != 0 ||
      sentinel_status.st_size != 4 || ::unlink(sentinel.c_str()) != 0) {
    return 1;
  }

  auto ready = make_ready(*opened.store, plan, {"abcd", ""});
  if (ready.error != AtomicDirectoryStoreError::none ||
      ready.transaction->publish_no_replace().error !=
          AtomicDirectoryStoreError::none ||
      ready.transaction->sync_published_parent() !=
          AtomicDirectoryStoreError::none) {
    std::cerr << "component: success\n";
    return 1;
  }
  return 0;
}

[[nodiscard]] int test_marker_last() {
  TemporaryRoot root;
  if (!root.valid()) {
    return capability_result(AtomicDirectoryStoreError::unsupported,
                             "writable marker fixture");
  }
  if (std::getenv("OHL_ATOMIC_STORE_TEST_FORCE_STORE_UNSUPPORTED") !=
      nullptr) {
    return capability_result(AtomicDirectoryStoreError::unsupported,
                             "forced marker store result");
  }
  auto opened = open_store(root);
  if (opened.error != AtomicDirectoryStoreError::none ||
      opened.store == nullptr) {
    return opened.error == AtomicDirectoryStoreError::unsupported
               ? capability_result(opened.error,
                                   "allow-listed marker filesystem")
               : 1;
  }
  PlanOwner plan{"marker", {{"f"}}, {1}};
  auto created = opened.store->create_transaction();
  if (created.transaction == nullptr ||
      created.transaction->begin(plan.plan()) != AtomicDirectoryStoreError::none) {
    return 1;
  }
  const auto stage = find_stage(root.path());
  if (stage.empty() ||
      exists(root.path() + "/" + stage + "/.ohl-complete-v1")) {
    return 1;
  }
  auto file = created.transaction->open_file(plan.views[0], 1);
  if (file.sink == nullptr ||
      file.sink->write(as_bytes("x")) != AtomicDirectoryStoreError::none ||
      created.transaction->seal_file(std::move(file.sink)) !=
          AtomicDirectoryStoreError::none ||
      exists(root.path() + "/" + stage + "/.ohl-complete-v1") ||
      created.transaction->seal_completion(plan.completion()) !=
          AtomicDirectoryStoreError::none) {
    return 1;
  }
  const auto marker_path =
      root.path() + "/" + stage + "/.ohl-complete-v1";
  const auto marker = ::open(marker_path.c_str(), O_RDONLY | O_CLOEXEC);
  std::array<std::byte, 128> data{};
  const auto count = marker < 0 ? -1 : ::read(marker, data.data(), data.size());
  if (marker >= 0) {
    (void)::close(marker);
  }
  const auto expected = expected_marker(plan);
  return count == static_cast<ssize_t>(expected.size()) &&
                 std::equal(expected.begin(), expected.end(), data.begin())
             ? 0
             : 1;
}

[[nodiscard]] int publish_plan(TemporaryRoot& root, PlanOwner& plan,
                               std::unique_ptr<AtomicDirectoryStore>& store) {
  auto opened = open_store(root);
  if (opened.error != AtomicDirectoryStoreError::none) {
    return 1;
  }
  store = std::move(opened.store);
  auto ready = make_ready(*store, plan, {"data"});
  if (ready.error != AtomicDirectoryStoreError::none ||
      ready.transaction->publish_no_replace().error !=
          AtomicDirectoryStoreError::none ||
      ready.transaction->sync_published_parent() !=
          AtomicDirectoryStoreError::none) {
    return 1;
  }
  return 0;
}

[[nodiscard]] int test_probe_exact_tree() {
  TemporaryRoot root;
  PlanOwner plan{"probe", {{"f"}}, {4}};
  std::unique_ptr<AtomicDirectoryStore> store;
  const auto published = publish_plan(root, plan, store);
  if (published != 0) {
    return published;
  }
  if (!probe_is(*store, plan, AtomicDirectoryProbeState::matching)) {
    return 1;
  }
  const auto final = root.path() + "/" + hex_identity(plan.identity);
  if (::mkdir((final + "/files/empty").c_str(), 0700) != 0 ||
      !probe_is(*store, plan, AtomicDirectoryProbeState::conflict) ||
      ::rmdir((final + "/files/empty").c_str()) != 0 ||
      !write_file(final + "/root-extra", "x") ||
      !probe_is(*store, plan, AtomicDirectoryProbeState::conflict) ||
      ::unlink((final + "/root-extra").c_str()) != 0 ||
      !write_file(final + "/files/extra", "x") ||
      !probe_is(*store, plan, AtomicDirectoryProbeState::conflict) ||
      ::unlink((final + "/files/extra").c_str()) != 0 ||
      !write_file(final + "/files/f", "zzzz") ||
      !probe_is(*store, plan, AtomicDirectoryProbeState::matching) ||
      ::truncate((final + "/files/f").c_str(), 2) != 0 ||
      !probe_is(*store, plan, AtomicDirectoryProbeState::conflict)) {
    return 1;
  }
  if (!write_file(final + "/files/f", "data") ||
      ::unlink((final + "/files/f").c_str()) != 0 ||
      !probe_is(*store, plan, AtomicDirectoryProbeState::conflict) ||
      ::mkdir((final + "/files/f").c_str(), 0700) != 0 ||
      !probe_is(*store, plan, AtomicDirectoryProbeState::conflict) ||
      ::rmdir((final + "/files/f").c_str()) != 0 ||
      ::symlink("outside", (final + "/files/f").c_str()) != 0 ||
      !probe_is(*store, plan, AtomicDirectoryProbeState::conflict) ||
      ::unlink((final + "/files/f").c_str()) != 0 ||
      !write_file(final + "/files/f", "data") ||
      ::link((final + "/files/f").c_str(),
             (final + "/files/hardlink").c_str()) != 0 ||
      !probe_is(*store, plan, AtomicDirectoryProbeState::conflict) ||
      ::unlink((final + "/files/hardlink").c_str()) != 0 ||
      !probe_is(*store, plan, AtomicDirectoryProbeState::matching) ||
      ::chmod((final + "/files/f").c_str(), 0660) != 0 ||
      !probe_is(*store, plan, AtomicDirectoryProbeState::conflict) ||
      ::chmod((final + "/files/f").c_str(), 0600) != 0 ||
      ::chmod((final + "/files").c_str(), 0777) != 0 ||
      !probe_is(*store, plan, AtomicDirectoryProbeState::conflict) ||
      ::chmod((final + "/files").c_str(), 0700) != 0) {
    return 1;
  }
  const auto marker = final + "/.ohl-complete-v1";
  const auto marker_backup = root.path() + "/marker-backup";
  const auto marker_hardlink = root.path() + "/marker-hardlink";
  if (::rename(marker.c_str(), marker_backup.c_str()) != 0 ||
      !probe_is(*store, plan, AtomicDirectoryProbeState::conflict) ||
      ::mkdir(marker.c_str(), 0700) != 0 ||
      !probe_is(*store, plan, AtomicDirectoryProbeState::conflict) ||
      ::rmdir(marker.c_str()) != 0 ||
      ::symlink(marker_backup.c_str(), marker.c_str()) != 0 ||
      !probe_is(*store, plan, AtomicDirectoryProbeState::conflict) ||
      ::unlink(marker.c_str()) != 0 ||
      ::rename(marker_backup.c_str(), marker.c_str()) != 0 ||
      ::link(marker.c_str(), marker_hardlink.c_str()) != 0 ||
      !probe_is(*store, plan, AtomicDirectoryProbeState::conflict) ||
      ::unlink(marker_hardlink.c_str()) != 0 ||
      !probe_is(*store, plan, AtomicDirectoryProbeState::matching)) {
    return 1;
  }

  const auto original_marker = expected_marker(plan);
  for (const auto offset : std::array<std::size_t, 5>{0, 15, 23, 31, 32}) {
    auto corrupted = original_marker;
    corrupted[offset] ^= std::byte{1};
    if (!write_bytes(marker, corrupted) ||
        !probe_is(*store, plan, AtomicDirectoryProbeState::conflict) ||
        !write_bytes(marker, original_marker)) {
      return 1;
    }
  }
  auto trailing = original_marker;
  trailing.push_back(std::byte{0});
  if (!write_bytes(marker, trailing) ||
      !probe_is(*store, plan, AtomicDirectoryProbeState::conflict) ||
      !write_bytes(marker, original_marker) ||
      !probe_is(*store, plan, AtomicDirectoryProbeState::matching)) {
    return 1;
  }

  const auto final_backup = root.path() + "/final-backup";
  if (::rename(final.c_str(), final_backup.c_str()) != 0 ||
      !write_file(final, "not-a-directory") ||
      !probe_is(*store, plan, AtomicDirectoryProbeState::conflict) ||
      ::unlink(final.c_str()) != 0 ||
      ::symlink(final_backup.c_str(), final.c_str()) != 0 ||
      !probe_is(*store, plan, AtomicDirectoryProbeState::conflict) ||
      ::unlink(final.c_str()) != 0 ||
      ::rename(final_backup.c_str(), final.c_str()) != 0 ||
      !probe_is(*store, plan, AtomicDirectoryProbeState::matching)) {
    return 1;
  }

  PlanOwner nested_plan{"probe-nested", {{"d", "f"}}, {4}};
  std::unique_ptr<AtomicDirectoryStore> nested_store;
  if (publish_plan(root, nested_plan, nested_store) != 0) {
    return 1;
  }
  const auto nested_final =
      root.path() + "/" + hex_identity(nested_plan.identity);
  const auto intermediate = nested_final + "/files/d";
  const auto intermediate_backup = nested_final + "/files/d-backup";
  if (::chmod(intermediate.c_str(), 0777) != 0 ||
      !probe_is(*nested_store, nested_plan,
                AtomicDirectoryProbeState::conflict) ||
      ::chmod(intermediate.c_str(), 0700) != 0 ||
      ::rename(intermediate.c_str(), intermediate_backup.c_str()) != 0 ||
      !write_file(intermediate, "wrong-type") ||
      !probe_is(*nested_store, nested_plan,
                AtomicDirectoryProbeState::conflict) ||
      ::unlink(intermediate.c_str()) != 0 ||
      ::rename(intermediate_backup.c_str(), intermediate.c_str()) != 0 ||
      !probe_is(*nested_store, nested_plan,
                AtomicDirectoryProbeState::matching)) {
    return 1;
  }
  return 0;
}

[[nodiscard]] int test_cleanup() {
  TemporaryRoot root;
  if (!root.valid()) {
    return 1;
  }
  auto opened = open_store(root);
  if (opened.error != AtomicDirectoryStoreError::none) {
    return 1;
  }
  PlanOwner plan{"cleanup", {{"f"}}, {1}};
  struct ReusedOwner {
    std::shared_ptr<ohl::platform::detail::NativeTransactionLifetime> lifetime;
  };
  alignas(ReusedOwner) std::array<std::byte, sizeof(ReusedOwner)> storage{};
  auto old_lifetime =
      std::make_shared<ohl::platform::detail::NativeTransactionLifetime>();
  old_lifetime->generation = 1;
  old_lifetime->open = true;
  auto* old_owner = ::new (storage.data()) ReusedOwner{old_lifetime};
  const auto* reused_address = static_cast<const void*>(old_owner);
  old_owner->~ReusedOwner();
  old_lifetime->owner_alive = false;
  auto new_lifetime =
      std::make_shared<ohl::platform::detail::NativeTransactionLifetime>();
  new_lifetime->generation = 1;
  new_lifetime->open = true;
  auto* new_owner = ::new (storage.data()) ReusedOwner{new_lifetime};
  const bool binding_is_distinct =
      reused_address == static_cast<const void*>(new_owner) &&
      !ohl::platform::detail::native_sink_binding_is_current(
          old_lifetime, 1, new_lifetime) &&
      ohl::platform::detail::native_sink_binding_is_current(
          new_lifetime, 1, new_lifetime);
  new_owner->~ReusedOwner();
  if (!binding_is_distinct) {
    return 1;
  }
  auto first = opened.store->create_transaction();
  auto second = opened.store->create_transaction();
  if (first.transaction->begin(plan.plan()) != AtomicDirectoryStoreError::none ||
      second.transaction->begin(plan.plan()) != AtomicDirectoryStoreError::none) {
    return 1;
  }
  auto sink = first.transaction->open_file(plan.views[0], 1);
  if (sink.sink == nullptr ||
      first.transaction->abort() != AtomicDirectoryStoreError::invalid_state) {
    return 1;
  }
  sink.sink.reset();
  if (first.transaction->abort() != AtomicDirectoryStoreError::none ||
      first.transaction->abort() != AtomicDirectoryStoreError::none ||
      second.transaction->abort() != AtomicDirectoryStoreError::none ||
      !find_stage(root.path()).empty()) {
    return 1;
  }

  auto lifetime = opened.store->create_transaction();
  if (lifetime.error != AtomicDirectoryStoreError::none ||
      lifetime.transaction == nullptr ||
      lifetime.transaction->begin(plan.plan()) !=
          AtomicDirectoryStoreError::none) {
    return 1;
  }
  auto orphaned = lifetime.transaction->open_file(plan.views[0], 1);
  if (orphaned.error != AtomicDirectoryStoreError::none ||
      orphaned.sink == nullptr) {
    return 1;
  }
  lifetime.transaction.reset();
  if (orphaned.sink->write(as_bytes("x")) !=
      AtomicDirectoryStoreError::invalid_state) {
    return 1;
  }
  auto current = opened.store->create_transaction();
  if (current.error != AtomicDirectoryStoreError::none ||
      current.transaction == nullptr ||
      current.transaction->begin(plan.plan()) !=
          AtomicDirectoryStoreError::none) {
    return 1;
  }
  auto current_file = current.transaction->open_file(plan.views[0], 1);
  if (current_file.error != AtomicDirectoryStoreError::none ||
      current_file.sink == nullptr ||
      current_file.sink->write(as_bytes("n")) !=
          AtomicDirectoryStoreError::none ||
      current.transaction->seal_file(std::move(orphaned.sink)) !=
          AtomicDirectoryStoreError::invalid_state ||
      current.transaction->seal_file(std::move(current_file.sink)) !=
          AtomicDirectoryStoreError::none ||
      current.transaction->seal_completion(plan.completion()) !=
          AtomicDirectoryStoreError::none) {
    return 1;
  }
  const auto publication = current.transaction->publish_no_replace();
  if (publication.error != AtomicDirectoryStoreError::none ||
      publication.state != AtomicDirectoryPublishState::published ||
      current.transaction->sync_published_parent() !=
          AtomicDirectoryStoreError::none ||
      !probe_is(*opened.store, plan, AtomicDirectoryProbeState::matching) ||
      read_bytes(root.path() + "/" + hex_identity(plan.identity) +
                 "/files/f") != as_bytes("n")) {
    return 1;
  }
  const auto retained_stage = find_stage(root.path());
  if (retained_stage.empty()) {
    return 1;
  }
  const auto retained_path = root.path() + "/" + retained_stage;
  if (::nftw(retained_path.c_str(), remove_item, 32, FTW_DEPTH | FTW_PHYS) != 0 ||
      !find_stage(root.path()).empty()) {
    return 1;
  }
  return 0;
}

[[nodiscard]] int test_publish_no_replace() {
  TemporaryRoot root;
  if (!root.valid()) {
    return 1;
  }
  auto opened = open_store(root);
  if (opened.error != AtomicDirectoryStoreError::none) {
    return opened.error == AtomicDirectoryStoreError::unsupported
               ? capability_result(opened.error, "allow-listed filesystem")
               : 1;
  }
  RemoveFileOnExit outside{root.path() + "-no-replace-sentinel"};
  if (!write_file(outside.path(), "outside-safe")) {
    return 1;
  }
  struct stat outside_before {};
  if (::stat(outside.path().c_str(), &outside_before) != 0) {
    return 1;
  }
  for (int kind = 0; kind < 4; ++kind) {
    PlanOwner plan{"no-replace-" + std::to_string(kind), {{"f"}}, {1}};
    auto ready = make_ready(*opened.store, plan, {"x"});
    if (ready.error != AtomicDirectoryStoreError::none) {
      return 1;
    }
    const auto target = root.path() + "/" + hex_identity(plan.identity);
    const auto created =
        kind == 0   ? write_file(target, "sentinel")
        : kind <= 2 ? ::mkdir(target.c_str(), 0700) == 0
                    : ::symlink(outside.path().c_str(), target.c_str()) == 0;
    if (!created) {
      return 1;
    }
    if (kind == 2 && !write_file(target + "/sentinel", "keep")) {
      return 1;
    }
    struct stat before {};
    struct stat after {};
    if (::lstat(target.c_str(), &before) != 0) {
      return 1;
    }
    const auto result = ready.transaction->publish_no_replace();
    if (result.error == AtomicDirectoryStoreError::unsupported) {
      (void)ready.transaction->abort();
      return capability_result(result.error, "renameat2 RENAME_NOREPLACE");
    }
    if (result.error != AtomicDirectoryStoreError::none ||
        result.state != AtomicDirectoryPublishState::destination_exists ||
        ready.transaction->abort() != AtomicDirectoryStoreError::none ||
        ::lstat(target.c_str(), &after) != 0 || before.st_dev != after.st_dev ||
        before.st_ino != after.st_ino || before.st_mode != after.st_mode ||
        !find_stage(root.path()).empty() ||
        read_bytes(outside.path()) != as_bytes("outside-safe")) {
      return 1;
    }
    if (kind == 0 && read_bytes(target) != as_bytes("sentinel")) {
      return 1;
    }
    if (kind == 2 && read_bytes(target + "/sentinel") != as_bytes("keep")) {
      return 1;
    }
    if (kind == 3) {
      std::array<char, 512> link_target{};
      const auto length =
          ::readlink(target.c_str(), link_target.data(), link_target.size());
      struct stat outside_after {};
      if (length != static_cast<ssize_t>(outside.path().size()) ||
          std::string_view{link_target.data(),
                           static_cast<std::size_t>(length)} != outside.path() ||
          ::stat(outside.path().c_str(), &outside_after) != 0 ||
          outside_before.st_dev != outside_after.st_dev ||
          outside_before.st_ino != outside_after.st_ino ||
          outside_before.st_mode != outside_after.st_mode ||
          read_bytes(outside.path()) != as_bytes("outside-safe") ||
          exists(root.path() + "/sentinel")) {
        return 1;
      }
    }
  }
  return 0;
}

[[nodiscard]] int race_child(const std::string& root, const bool conflict,
                             const int ready_fd, const int release_fd,
                             const int result_fd) {
  auto opened = ohl::platform::open_atomic_directory_store(root);
  if (opened.error != AtomicDirectoryStoreError::none ||
      opened.store == nullptr) {
    return 1;
  }
  PlanOwner plan{"race", {{conflict ? "b" : "a"}}, {1}};
  auto ready = make_ready(*opened.store, plan, {conflict ? "y" : "x"});
  const char signal = 'R';
  if (ready.error != AtomicDirectoryStoreError::none ||
      ::write(ready_fd, &signal, 1) != 1) {
    return 1;
  }
  char release{};
  if (!read_full(release_fd, std::span{&release, 1})) {
    return 1;
  }
  const auto publication = ready.transaction->publish_no_replace();
  char outcome = 'E';
  if (publication.error == AtomicDirectoryStoreError::none &&
      publication.state == AtomicDirectoryPublishState::published) {
    outcome = 'P';
  } else if (publication.error == AtomicDirectoryStoreError::none &&
             publication.state == AtomicDirectoryPublishState::destination_exists) {
    if (ready.transaction->abort() != AtomicDirectoryStoreError::none) {
      return 1;
    }
    const auto probe = opened.store->probe(plan.plan());
    if (probe.error == AtomicDirectoryStoreError::none) {
      outcome = probe.state == AtomicDirectoryProbeState::matching ? 'M'
                : probe.state == AtomicDirectoryProbeState::conflict ? 'C'
                                                                    : 'A';
    }
  }
  return ::write(result_fd, &outcome, 1) == 1 ? 0 : 1;
}

[[nodiscard]] bool wait_child(const pid_t child, int& status) {
  for (int attempt = 0; attempt < 500; ++attempt) {
    const auto result = ::waitpid(child, &status, WNOHANG);
    if (result == child) {
      return true;
    }
    if (result < 0 && errno != EINTR) {
      return false;
    }
    (void)::poll(nullptr, 0, 10);
  }
  return false;
}

[[nodiscard]] bool kill_and_reap(const pid_t child) {
  if (child <= 0) {
    return true;
  }
  int status = 0;
  while (true) {
    const auto result = ::waitpid(child, &status, WNOHANG);
    if (result == child || (result < 0 && errno == ECHILD)) {
      return true;
    }
    if (result == 0) {
      break;
    }
    if (errno != EINTR) {
      return false;
    }
  }
  if (::kill(child, SIGKILL) != 0 && errno != ESRCH) {
    return false;
  }
  return wait_child(child, status);
}

void close_descriptor(int& descriptor) {
  if (descriptor >= 0) {
    (void)::close(descriptor);
    descriptor = -1;
  }
}

void close_pipe(std::array<int, 2>& pipe) {
  close_descriptor(pipe[0]);
  close_descriptor(pipe[1]);
}

[[nodiscard]] int test_race(const bool conflicting) {
  TemporaryRoot root;
  if (!root.valid()) {
    return capability_result(AtomicDirectoryStoreError::unsupported,
                             "writable tmpfs race fixture");
  }
  auto capability = open_store(root);
  if (capability.error != AtomicDirectoryStoreError::none) {
    return capability.error == AtomicDirectoryStoreError::unsupported
               ? capability_result(capability.error,
                                   "allow-listed race filesystem")
               : 1;
  }
  RemoveFileOnExit outside_sentinel{root.path() + "-race-sentinel"};
  if (!write_file(outside_sentinel.path(), "safe")) {
    return 1;
  }
  std::array<int, 2> ready_pipe{-1, -1};
  std::array<int, 2> release_one{-1, -1};
  std::array<int, 2> release_two{-1, -1};
  std::array<int, 2> result_pipe{-1, -1};
  const auto close_all_pipes = [&] {
    close_pipe(ready_pipe);
    close_pipe(release_one);
    close_pipe(release_two);
    close_pipe(result_pipe);
  };
  if (::pipe2(ready_pipe.data(), O_CLOEXEC) != 0 ||
      ::pipe2(release_one.data(), O_CLOEXEC) != 0 ||
      ::pipe2(release_two.data(), O_CLOEXEC) != 0 ||
      ::pipe2(result_pipe.data(), O_CLOEXEC) != 0) {
    close_all_pipes();
    return 1;
  }
  const auto first = ::fork();
  if (first == 0) {
    (void)::close(ready_pipe[0]);
    (void)::close(release_one[1]);
    (void)::close(release_two[0]);
    (void)::close(release_two[1]);
    (void)::close(result_pipe[0]);
    _exit(race_child(root.path(), false, ready_pipe[1], release_one[0],
                     result_pipe[1]));
  }
  if (first < 0) {
    close_all_pipes();
    return 1;
  }
  const auto second = ::fork();
  if (second == 0) {
    (void)::close(ready_pipe[0]);
    (void)::close(release_one[0]);
    (void)::close(release_one[1]);
    (void)::close(release_two[1]);
    (void)::close(result_pipe[0]);
    _exit(race_child(root.path(), conflicting, ready_pipe[1], release_two[0],
                     result_pipe[1]));
  }
  if (second < 0) {
    close_all_pipes();
    (void)kill_and_reap(first);
    return 1;
  }
  close_descriptor(ready_pipe[1]);
  close_descriptor(release_one[0]);
  close_descriptor(release_two[0]);
  close_descriptor(result_pipe[1]);

  const auto fail = [&] {
    close_all_pipes();
    (void)kill_and_reap(first);
    (void)kill_and_reap(second);
    return 1;
  };
  char ready[2]{};
  if (!read_full(ready_pipe[0], ready) ||
      ::write(release_one[1], "x", 1) != 1 ||
      ::write(release_two[1], "x", 1) != 1) {
    return fail();
  }
  close_descriptor(ready_pipe[0]);
  close_descriptor(release_one[1]);
  close_descriptor(release_two[1]);
  char outcomes[2]{};
  if (!read_full(result_pipe[0], outcomes)) {
    return fail();
  }
  close_descriptor(result_pipe[0]);
  int first_status = 0;
  int second_status = 0;
  if (!wait_child(first, first_status)) {
    return fail();
  }
  if (!wait_child(second, second_status)) {
    (void)kill_and_reap(second);
    return 1;
  }
  const std::string_view observed{outcomes, 2};
  const auto expected_loser = conflicting ? 'C' : 'M';
  const bool process_result =
      WIFEXITED(first_status) && WEXITSTATUS(first_status) == 0 &&
      WIFEXITED(second_status) && WEXITSTATUS(second_status) == 0 &&
      std::count(observed.begin(), observed.end(), 'P') == 1 &&
      std::count(observed.begin(), observed.end(), expected_loser) == 1;
  PlanOwner first_plan{"race", {{"a"}}, {1}};
  PlanOwner second_plan{"race", {{conflicting ? "b" : "a"}}, {1}};
  const auto first_probe = capability.store->probe(first_plan.plan());
  const auto second_probe = capability.store->probe(second_plan.plan());
  const auto final = root.path() + "/" + hex_identity("race") + "/files/";
  const bool exact_result =
      !conflicting
          ? first_probe.error == AtomicDirectoryStoreError::none &&
                first_probe.state == AtomicDirectoryProbeState::matching &&
                read_bytes(final + "a") == as_bytes("x")
          : (first_probe.error == AtomicDirectoryStoreError::none &&
             second_probe.error == AtomicDirectoryStoreError::none &&
             ((first_probe.state == AtomicDirectoryProbeState::matching &&
              second_probe.state == AtomicDirectoryProbeState::conflict &&
              read_bytes(final + "a") == as_bytes("x")) ||
             (second_probe.state == AtomicDirectoryProbeState::matching &&
              first_probe.state == AtomicDirectoryProbeState::conflict &&
              read_bytes(final + "b") == as_bytes("y"))));
  const bool sentinel_safe =
      read_bytes(outside_sentinel.path()) == as_bytes("safe");
  return process_result && exact_result && sentinel_safe &&
                 find_stage(root.path()).empty()
             ? 0
             : 1;
}

class InjectedOps final : public ohl::platform::detail::NativeOps {
 public:
  enum class StatMutation { none, wrong_owner, world_writable, wrong_device };

  explicit InjectedOps(ohl::platform::detail::NativeOps& base) : base_(base) {}
  int open_root() noexcept override {
    const auto descriptor = base_.open_root();
    if (descriptor >= 0) {
      roles[descriptor] = "root";
    }
    return descriptor;
  }
  int open_at(int d, const char* n, int f, mode_t m) noexcept override {
    if (fail_next_open) {
      fail_next_open = false;
      errno = EIO;
      return -1;
    }
    const auto descriptor = base_.open_at(d, n, f, m);
    if (descriptor >= 0) {
      const auto parent = roles.contains(d) ? roles[d] : "unknown";
      std::string role;
      if (std::string_view{n}.starts_with(".ohl-stage-")) {
        role = "stage";
      } else if (std::string_view{n}.starts_with("ohl-tree-")) {
        role = "final";
      } else if (std::string_view{n} == "files") {
        role = "files";
      } else if (std::string_view{n} == ".ohl-complete-v1") {
        role = "marker";
      } else if ((f & O_DIRECTORY) != 0) {
        role = parent == "root" ? "root" : "nested";
      } else {
        role = "file";
      }
      roles[descriptor] = std::move(role);
    }
    return descriptor;
  }
  int make_directory_at(int d, const char* n, mode_t m) noexcept override {
    if (fail_next_mkdir) {
      fail_next_mkdir = false;
      errno = EIO;
      return -1;
    }
    return base_.make_directory_at(d, n, m);
  }
  int unlink_at(int d, const char* n, int f) noexcept override {
    events.emplace_back("unlink:" + std::string{n});
    if (fail_next_unlink) {
      fail_next_unlink = false;
      errno = EIO;
      return -1;
    }
    return base_.unlink_at(d, n, f);
  }
  int duplicate(int d) noexcept override {
    const auto duplicate = base_.duplicate(d);
    if (duplicate >= 0 && roles.contains(d)) {
      roles[duplicate] = roles[d];
    }
    return duplicate;
  }
  int close_fd(int d) noexcept override {
    events.emplace_back("close:" + role(d));
    roles.erase(d);
    if (fail_next_close) {
      fail_next_close = false;
      (void)base_.close_fd(d);
      errno = EIO;
      return -1;
    }
    return base_.close_fd(d);
  }
  int stat_fd(int d, struct stat* s) noexcept override {
    const auto result = base_.stat_fd(d, s);
    if (result != 0 || role(d) != mutated_stat_role) {
      return result;
    }
    switch (stat_mutation) {
      case StatMutation::wrong_owner:
        s->st_uid = static_cast<uid_t>(geteuid() ^ 1U);
        break;
      case StatMutation::world_writable:
        s->st_mode |= S_IRWXG | S_IRWXO;
        break;
      case StatMutation::wrong_device:
        s->st_dev = static_cast<dev_t>(s->st_dev + 1);
        break;
      case StatMutation::none:
        break;
    }
    return result;
  }
  int stat_at(int d, const char* n, struct stat* s, int f) noexcept override {
    return base_.stat_at(d, n, s, f);
  }
  int stat_filesystem(int d, struct statfs* s) noexcept override {
    return base_.stat_filesystem(d, s);
  }
  bool supported_filesystem(const struct statfs& s) noexcept override {
    return base_.supported_filesystem(s);
  }
  long name_max(int d) noexcept override {
    if (name_max_indeterminate) {
      errno = 0;
      return -1;
    }
    if (name_max_error != 0) {
      errno = name_max_error;
      return -1;
    }
    return base_.name_max(d);
  }
  ssize_t read_fd(int d, void* p, std::size_t s) noexcept override {
    if (read_eintr) {
      read_eintr = false;
      errno = EINTR;
      return -1;
    }
    const auto amount = partial_reads && s > 1 ? s / 2 : s;
    return base_.read_fd(d, p, amount);
  }
  ssize_t write_fd(int d, const void* p, std::size_t s) noexcept override {
    events.emplace_back("write:" + role(d));
    if (write_eintr) {
      write_eintr = false;
      errno = EINTR;
      return -1;
    }
    const auto amount = partial_writes && s > 1 ? s / 2 : s;
    return base_.write_fd(d, p, amount);
  }
  int sync_fd(int d) noexcept override {
    events.emplace_back("fsync:" + role(d));
    ++sync_calls;
    if (fail_sync_call != 0 && sync_calls == fail_sync_call) {
      errno = EIO;
      return -1;
    }
    if (fail_next_sync) {
      fail_next_sync = false;
      errno = EIO;
      return -1;
    }
    return base_.sync_fd(d);
  }
  ssize_t random_bytes(void* p, std::size_t s) noexcept override {
    if (random_error) {
      errno = EIO;
      return -1;
    }
    if (fixed_random) {
      std::memset(p, 0, s);
      if (s > 0) {
        static_cast<unsigned char*>(p)[s - 1] =
            static_cast<unsigned char>(random_index++);
      }
      return static_cast<ssize_t>(s);
    }
    if (random_eintr) {
      random_eintr = false;
      errno = EINTR;
      return -1;
    }
    return base_.random_bytes(p, s > 3 ? 3 : s);
  }
  int rename_no_replace(int a, const char* b, int c,
                        const char* d) noexcept override {
    events.emplace_back("rename");
    if (rename_errno != 0) {
      errno = rename_errno;
      return -1;
    }
    if (rename_error_without_action) {
      errno = EIO;
      return -1;
    }
    if (rename_success_with_error) {
      const auto result = base_.rename_no_replace(a, b, c, d);
      if (result == 0) {
        errno = EIO;
        return -1;
      }
      return result;
    }
    return base_.rename_no_replace(a, b, c, d);
  }
  DIR* open_directory_stream(int d) noexcept override {
    return base_.open_directory_stream(d);
  }
  struct dirent* read_directory(DIR* d) noexcept override {
    return base_.read_directory(d);
  }
  int close_directory(DIR* d) noexcept override {
    return base_.close_directory(d);
  }

  [[nodiscard]] std::string role(const int descriptor) const {
    const auto found = roles.find(descriptor);
    return found == roles.end() ? "unknown" : found->second;
  }

  bool random_eintr{true};
  bool write_eintr{true};
  bool partial_writes{true};
  bool read_eintr{false};
  bool partial_reads{false};
  bool fail_next_sync{false};
  bool fail_next_open{false};
  bool fail_next_mkdir{false};
  std::size_t sync_calls{0};
  std::size_t fail_sync_call{0};
  bool fail_next_close{false};
  bool fail_next_unlink{false};
  bool random_error{false};
  bool fixed_random{false};
  unsigned int random_index{0};
  int rename_errno{0};
  bool rename_error_without_action{false};
  bool rename_success_with_error{false};
  bool name_max_indeterminate{false};
  int name_max_error{0};
  StatMutation stat_mutation{StatMutation::none};
  std::string mutated_stat_role;
  std::vector<std::string> events;
  std::map<int, std::string> roles;

 private:
  ohl::platform::detail::NativeOps& base_;
};

[[nodiscard]] int test_fault_injection() {
  TemporaryRoot root;
  if (!root.valid()) {
    return 1;
  }
  {
    InjectedOps indeterminate{ohl::platform::detail::linux_native_ops()};
    indeterminate.name_max_indeterminate = true;
    const auto result =
        ohl::platform::detail::open_atomic_directory_store_with_ops(root.path(),
                                                                    indeterminate);
    if (result.error != AtomicDirectoryStoreError::unsupported || result.store) {
      return 1;
    }
  }
  {
    InjectedOps exhausted{ohl::platform::detail::linux_native_ops()};
    exhausted.name_max_error = EMFILE;
    const auto result =
        ohl::platform::detail::open_atomic_directory_store_with_ops(root.path(),
                                                                    exhausted);
    if (result.error != AtomicDirectoryStoreError::resource_exhausted ||
        result.store) {
      return 1;
    }
  }
  InjectedOps ops{ohl::platform::detail::linux_native_ops()};
  struct statfs ext_family_status {};
  ext_family_status.f_type = kExtFamilySuperMagic;
  if (!ops.supported_filesystem(ext_family_status)) {
    return 1;
  }
  auto opened = ohl::platform::detail::open_atomic_directory_store_with_ops(
      root.path(), ops);
  if (opened.error != AtomicDirectoryStoreError::none) {
    return 1;
  }
  PlanOwner plan{"fault", {{"nested", "f"}}, {4}};

  ops.fail_next_mkdir = true;
  auto mkdir_failure = opened.store->create_transaction();
  if (mkdir_failure.transaction->begin(plan.plan()) !=
          AtomicDirectoryStoreError::io_failure ||
      mkdir_failure.transaction->abort() != AtomicDirectoryStoreError::none) {
    return 1;
  }
  ops.fail_next_open = true;
  auto open_failure = opened.store->create_transaction();
  if (open_failure.transaction->begin(plan.plan()) !=
          AtomicDirectoryStoreError::io_failure ||
      open_failure.transaction->abort() != AtomicDirectoryStoreError::none) {
    return 1;
  }

  for (const auto rename_error : {ENOSYS, EINVAL, EOPNOTSUPP}) {
    PlanOwner unsupported_plan{"unsupported-" + std::to_string(rename_error),
                               {{"f"}}, {1}};
    auto unsupported = make_ready(*opened.store, unsupported_plan, {"x"});
    ops.rename_errno = rename_error;
    if (unsupported.error != AtomicDirectoryStoreError::none ||
        unsupported.transaction->publish_no_replace().error !=
            AtomicDirectoryStoreError::unsupported ||
        unsupported.transaction->abort() != AtomicDirectoryStoreError::none) {
      return 1;
    }
  }
  ops.rename_errno = 0;
  for (std::size_t failed_sync = 1; failed_sync <= 5; ++failed_sync) {
    ops.sync_calls = 0;
    ops.fail_sync_call = failed_sync;
    auto sync_failure = make_ready(*opened.store, plan, {"data"});
    if (sync_failure.error != AtomicDirectoryStoreError::io_failure ||
        sync_failure.transaction->abort() != AtomicDirectoryStoreError::none) {
      return 1;
    }
  }
  ops.fail_sync_call = 0;
  ops.sync_calls = 0;
  ops.events.clear();

  auto second = make_ready(*opened.store, plan, {"data"});
  if (second.error != AtomicDirectoryStoreError::none || ops.sync_calls != 5) {
    return 1;
  }
  const auto second_publication = second.transaction->publish_no_replace();
  if (second_publication.error != AtomicDirectoryStoreError::none ||
      second_publication.state != AtomicDirectoryPublishState::published) {
    return 1;
  }
  ops.fail_next_sync = true;
  if (second.transaction->sync_published_parent() !=
          AtomicDirectoryStoreError::io_failure ||
      ops.sync_calls != 6) {
    return 1;
  }
  if (second.transaction->sync_published_parent() !=
      AtomicDirectoryStoreError::none) {
    return 1;
  }
  const std::array expected_events{
      std::string{"fsync:file"},   std::string{"close:file"},
      std::string{"write:marker"}, std::string{"fsync:marker"},
      std::string{"close:marker"},
      std::string{"fsync:nested"}, std::string{"close:nested"},
      std::string{"fsync:files"},  std::string{"close:files"},
      std::string{"fsync:stage"},  std::string{"rename"},
      std::string{"fsync:root"}};
  auto next_event = ops.events.cbegin();
  for (const auto& expected : expected_events) {
    next_event = std::find(next_event, ops.events.cend(), expected);
    if (next_event == ops.events.cend()) {
      return 1;
    }
    ++next_event;
  }

  const auto expect_structural_conflict = [&](const std::string_view role,
                                               const InjectedOps::StatMutation mutation) {
    ops.mutated_stat_role = role;
    ops.stat_mutation = mutation;
    const auto probe = opened.store->probe(plan.plan());
    ops.stat_mutation = InjectedOps::StatMutation::none;
    ops.mutated_stat_role.clear();
    return probe.error == AtomicDirectoryStoreError::none &&
           probe.state == AtomicDirectoryProbeState::conflict;
  };
  if (!expect_structural_conflict(
          "files", InjectedOps::StatMutation::wrong_owner) ||
      !expect_structural_conflict(
          "files", InjectedOps::StatMutation::world_writable) ||
      !expect_structural_conflict(
          "files", InjectedOps::StatMutation::wrong_device) ||
      !expect_structural_conflict(
          "nested", InjectedOps::StatMutation::wrong_owner) ||
      !expect_structural_conflict(
          "nested", InjectedOps::StatMutation::world_writable) ||
      !expect_structural_conflict(
          "nested", InjectedOps::StatMutation::wrong_device) ||
      !probe_is(*opened.store, plan, AtomicDirectoryProbeState::matching)) {
    return 1;
  }

  PlanOwner reconciled_plan{"reconciled", {{"f"}}, {1}};
  auto reconciled = make_ready(*opened.store, reconciled_plan, {"x"});
  ops.rename_success_with_error = true;
  const auto reconciled_publication =
      reconciled.transaction->publish_no_replace();
  if (reconciled.error != AtomicDirectoryStoreError::none ||
      reconciled_publication.error != AtomicDirectoryStoreError::none ||
      reconciled_publication.state != AtomicDirectoryPublishState::published ||
      reconciled.transaction->abort() !=
          AtomicDirectoryStoreError::invalid_state ||
      reconciled.transaction->sync_published_parent() !=
          AtomicDirectoryStoreError::none ||
      opened.store->probe(reconciled_plan.plan()).state !=
          AtomicDirectoryProbeState::matching ||
      !find_stage(root.path()).empty()) {
    return 1;
  }
  ops.rename_success_with_error = false;

  ops.read_eintr = true;
  ops.partial_reads = true;
  const auto partial_probe = opened.store->probe(reconciled_plan.plan());
  ops.partial_reads = false;
  if (partial_probe.error != AtomicDirectoryStoreError::none ||
      partial_probe.state != AtomicDirectoryProbeState::matching) {
    return 1;
  }
  ops.fail_next_open = true;
  const auto failed_probe = opened.store->probe(reconciled_plan.plan());
  if (failed_probe.error != AtomicDirectoryStoreError::io_failure ||
      failed_probe.state != AtomicDirectoryProbeState::absent) {
    return 1;
  }

  for (const auto rename_error : {EXDEV, EIO}) {
    PlanOwner failed_plan{"rename-error-" + std::to_string(rename_error),
                          {{"f"}}, {1}};
    auto failed = make_ready(*opened.store, failed_plan, {"x"});
    ops.rename_errno = rename_error;
    if (failed.error != AtomicDirectoryStoreError::none ||
        failed.transaction->publish_no_replace().error !=
            AtomicDirectoryStoreError::io_failure ||
        failed.transaction->abort() != AtomicDirectoryStoreError::none ||
        opened.store->probe(failed_plan.plan()).state !=
            AtomicDirectoryProbeState::absent) {
      return 1;
    }
  }
  ops.rename_errno = 0;

  ops.random_error = true;
  auto entropy_failure = opened.store->create_transaction();
  if (entropy_failure.transaction->begin(plan.plan()) !=
          AtomicDirectoryStoreError::io_failure ||
      entropy_failure.transaction->abort() != AtomicDirectoryStoreError::none) {
    return 1;
  }
  ops.random_error = false;
  ops.fixed_random = true;
  const auto collision_name = [&](const unsigned int index) {
    constexpr char hex[] = "0123456789abcdef";
    std::string name{".ohl-stage-"};
    name.append(30, '0');
    name.push_back(hex[(index >> 4U) & 0xfU]);
    name.push_back(hex[index & 0xfU]);
    return root.path() + "/" + name;
  };
  for (unsigned int index = 0; index < 15; ++index) {
    if (::mkdir(collision_name(index).c_str(), 0700) != 0) {
      return 1;
    }
  }
  ops.random_index = 0;
  auto fifteenth_collision = opened.store->create_transaction();
  if (fifteenth_collision.transaction->begin(plan.plan()) !=
          AtomicDirectoryStoreError::none ||
      fifteenth_collision.transaction->abort() !=
          AtomicDirectoryStoreError::none) {
    return 1;
  }
  if (::mkdir(collision_name(15).c_str(), 0700) != 0) {
    return 1;
  }
  ops.random_index = 0;
  auto sixteenth_collision = opened.store->create_transaction();
  if (sixteenth_collision.transaction->begin(plan.plan()) !=
          AtomicDirectoryStoreError::resource_exhausted ||
      sixteenth_collision.transaction->abort() !=
          AtomicDirectoryStoreError::none) {
    return 1;
  }
  for (unsigned int index = 0; index < 16; ++index) {
    if (::rmdir(collision_name(index).c_str()) != 0) {
      return 1;
    }
  }
  ops.fixed_random = false;

  auto close_failure = opened.store->create_transaction();
  if (close_failure.transaction->begin(plan.plan()) !=
      AtomicDirectoryStoreError::none) {
    return 1;
  }
  auto file = close_failure.transaction->open_file(plan.views[0], 4);
  if (file.sink == nullptr ||
      file.sink->write(as_bytes("data")) != AtomicDirectoryStoreError::none) {
    return 1;
  }
  ops.fail_next_close = true;
  if (close_failure.transaction->seal_file(std::move(file.sink)) !=
          AtomicDirectoryStoreError::io_failure ||
      close_failure.transaction->abort() != AtomicDirectoryStoreError::none) {
    return 1;
  }

  auto unlink_failure = opened.store->create_transaction();
  if (unlink_failure.transaction->begin(plan.plan()) !=
      AtomicDirectoryStoreError::none) {
    return 1;
  }
  const auto outside_sentinel = root.path() + "-cleanup-sentinel";
  if (!write_file(outside_sentinel, "safe")) {
    return 1;
  }
  ops.fail_next_unlink = true;
  const auto cleanup_error = unlink_failure.transaction->abort();
  const auto leaked_stage = find_stage(root.path());
  const bool confined = read_bytes(outside_sentinel) == as_bytes("safe") &&
                        !leaked_stage.empty();
  if (!leaked_stage.empty()) {
    const auto stage_path = root.path() + "/" + leaked_stage;
    (void)::nftw(stage_path.c_str(), remove_item, 32, FTW_DEPTH | FTW_PHYS);
  }
  (void)::unlink(outside_sentinel.c_str());
  return cleanup_error == AtomicDirectoryStoreError::io_failure && confined &&
                 find_stage(root.path()).empty()
             ? 0
             : 1;
}

}  // namespace

int main(const int argument_count, char** arguments) {
  if (argument_count != 2) {
    return 1;
  }
  const std::string_view name{arguments[1]};
  const bool publication_dependent =
      name == "root_security" || name == "component_io" ||
      name == "probe_exact_tree" || name == "cleanup" ||
      name == "publish_no_replace" || name == "race_matching" ||
      name == "race_conflicting" || name == "fault_injection";
  if (publication_dependent) {
    const auto capability_preflight = publication_capability_preflight();
    if (capability_preflight != 0) {
      return capability_preflight;
    }
  }
  if (name == "root_security") return test_root_security();
  if (name == "component_io") return test_component_io();
  if (name == "marker_last") return test_marker_last();
  if (name == "probe_exact_tree") return test_probe_exact_tree();
  if (name == "cleanup") return test_cleanup();
  if (name == "publish_no_replace") return test_publish_no_replace();
  if (name == "race_matching") return test_race(false);
  if (name == "race_conflicting") return test_race(true);
  if (name == "fault_injection") return test_fault_injection();
  return 1;
}
