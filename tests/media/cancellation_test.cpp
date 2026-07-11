#include "ohl/media/cancellation.hpp"

#include <algorithm>
#include <array>
#include <atomic>
#include <chrono>
#include <cstddef>
#include <iostream>
#include <thread>
#include <type_traits>
#include <utility>

namespace {

using ohl::media::CancellationSource;
using ohl::media::CancellationToken;

static_assert(std::is_nothrow_default_constructible_v<CancellationToken>);
static_assert(std::is_nothrow_copy_constructible_v<CancellationToken>);
static_assert(std::is_nothrow_copy_assignable_v<CancellationToken>);
static_assert(std::is_nothrow_move_constructible_v<CancellationToken>);
static_assert(std::is_nothrow_move_assignable_v<CancellationToken>);
static_assert(std::is_nothrow_copy_constructible_v<CancellationSource>);
static_assert(std::is_nothrow_copy_assignable_v<CancellationSource>);
static_assert(std::is_nothrow_move_constructible_v<CancellationSource>);
static_assert(std::is_nothrow_move_assignable_v<CancellationSource>);
static_assert(noexcept(std::declval<const CancellationToken&>().stop_possible()));
static_assert(noexcept(std::declval<const CancellationToken&>().stop_requested()));
static_assert(noexcept(std::declval<const CancellationSource&>().get_token()));
static_assert(noexcept(std::declval<const CancellationSource&>().stop_possible()));
static_assert(noexcept(std::declval<const CancellationSource&>().stop_requested()));
static_assert(noexcept(std::declval<CancellationSource&>().request_stop()));

[[nodiscard]] bool fail(const char* const message) {
  std::cerr << message << '\n';
  return false;
}

[[nodiscard]] bool test_default_token() {
  const CancellationToken first;
  const CancellationToken second;
  return !first.stop_possible() && !first.stop_requested() && first == second
             ? true
             : fail("default cancellation token contract failed");
}

[[nodiscard]] bool test_value_semantics_and_identity() {
  CancellationSource source;
  CancellationSource distinct;
  const auto token = source.get_token();
  const auto distinct_token = distinct.get_token();
  if (!source.stop_possible() || source.stop_requested() ||
      !token.stop_possible() || token.stop_requested() || source == distinct ||
      token == distinct_token || token == CancellationToken{}) {
    return fail("fresh cancellation identity contract failed");
  }

  CancellationToken copied_token{token};
  CancellationToken copy_assigned_token;
  copy_assigned_token = token;
  CancellationToken moved_token{std::move(copied_token)};
  CancellationToken move_assigned_token;
  move_assigned_token = std::move(copy_assigned_token);
  if (moved_token != token || move_assigned_token != token ||
      !moved_token.stop_possible() || moved_token.stop_requested()) {
    return fail("token copy/move/assignment lost shared identity");
  }

  CancellationSource copied_source{source};
  CancellationSource copy_assigned_source;
  const auto retired_copy_assignment = copy_assigned_source.get_token();
  copy_assigned_source = source;
  if (copied_source != source || copy_assigned_source != source ||
      retired_copy_assignment.stop_possible()) {
    return fail("source copy/assignment identity or retirement failed");
  }

  CancellationSource moved_source{std::move(copied_source)};
  CancellationSource move_assigned_source;
  const auto retired_move_assignment = move_assigned_source.get_token();
  move_assigned_source = std::move(moved_source);
  if (copied_source.stop_possible() || copied_source.stop_requested() ||
      copied_source.request_stop() || moved_source.stop_possible() ||
      moved_source.request_stop() || move_assigned_source != source ||
      retired_move_assignment.stop_possible()) {
    return fail("source move/assignment contract failed");
  }

  CancellationSource empty_copy{copied_source};
  CancellationSource emptied_assignment;
  const auto retired_empty_assignment = emptied_assignment.get_token();
  emptied_assignment = copied_source;
  if (empty_copy.stop_possible() || emptied_assignment.stop_possible() ||
      empty_copy != copied_source || emptied_assignment != copied_source ||
      retired_empty_assignment.stop_possible()) {
    return fail("copying or assigning a moved-from source recreated state");
  }

  if (!move_assigned_source.request_stop() ||
      move_assigned_source.request_stop() || source.request_stop() ||
      !source.stop_requested() || !copy_assigned_source.stop_requested() ||
      !token.stop_requested() || !moved_token.stop_requested() ||
      !move_assigned_token.stop_requested()) {
    return fail("shared idempotent stop request contract failed");
  }
  return true;
}

[[nodiscard]] bool test_token_lifetime_after_sources() {
  CancellationToken unstopped;
  {
    CancellationSource source;
    CancellationSource copy{source};
    unstopped = source.get_token();
    if (!unstopped.stop_possible()) {
      return fail("live copied source was not reflected by its token");
    }
  }
  if (unstopped.stop_possible() || unstopped.stop_requested()) {
    return fail("unstopped token remained possible after its last source");
  }

  CancellationToken requested;
  {
    CancellationSource source;
    requested = source.get_token();
    if (!source.request_stop()) {
      return fail("first lifetime stop request was rejected");
    }
  }
  return requested.stop_possible() && requested.stop_requested()
             ? true
             : fail("requested token lost state after source destruction");
}

[[nodiscard]] bool test_cross_thread_request_and_observation() {
  CancellationSource source;
  const auto token = source.get_token();
  std::atomic<bool> observer_ready{false};
  bool observed{false};
  std::thread observer{[token, &observer_ready, &observed]() {
    observer_ready.store(true, std::memory_order_release);
    const auto deadline =
        std::chrono::steady_clock::now() + std::chrono::seconds{5};
    while (!token.stop_requested() &&
           std::chrono::steady_clock::now() < deadline) {
      std::this_thread::yield();
    }
    observed = token.stop_possible() && token.stop_requested();
  }};
  while (!observer_ready.load(std::memory_order_acquire)) {
    std::this_thread::yield();
  }
  const bool first_request = source.request_stop();
  observer.join();
  if (!first_request || !observed) {
    return fail("cross-thread stop request was not observed");
  }

  CancellationSource contested;
  constexpr std::size_t kWorkers = 8;
  std::array<bool, kWorkers> won{};
  std::array<std::thread, kWorkers> workers;
  for (std::size_t index = 0; index < workers.size(); ++index) {
    workers[index] = std::thread{[copy = contested, &won, index]() mutable {
      won[index] = copy.request_stop();
    }};
  }
  for (auto& worker : workers) {
    worker.join();
  }
  const auto winners = std::count(won.begin(), won.end(), true);
  return winners == 1 && contested.stop_requested() &&
                 contested.get_token().stop_requested()
             ? true
             : fail("concurrent stop requests were not globally idempotent");
}

}  // namespace

int main() {
  return test_default_token() && test_value_semantics_and_identity() &&
                 test_token_lifetime_after_sources() &&
                 test_cross_thread_request_and_observation()
             ? 0
             : 1;
}
