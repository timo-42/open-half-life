#include "ohl/media/cancellation.hpp"

#include <atomic>
#include <cstddef>
#include <memory>
#include <utility>

namespace ohl::media::detail {

class CancellationState final {
 public:
  std::atomic<bool> stop_requested{false};
  std::atomic<std::size_t> source_count{1};
};

}  // namespace ohl::media::detail

namespace ohl::media {

CancellationToken::CancellationToken(
    std::shared_ptr<detail::CancellationState> state) noexcept
    : state_{std::move(state)} {}

bool CancellationToken::stop_possible() const noexcept {
  if (state_ == nullptr) {
    return false;
  }
  // Once the source count reaches zero no new stop request can begin. Reading
  // the final flag afterward therefore closes the last-source race.
  return state_->source_count.load(std::memory_order_acquire) != 0 ||
         state_->stop_requested.load(std::memory_order_acquire);
}

bool CancellationToken::stop_requested() const noexcept {
  return state_ != nullptr &&
         state_->stop_requested.load(std::memory_order_acquire);
}

bool operator==(const CancellationToken& first,
                const CancellationToken& second) noexcept {
  return first.state_ == second.state_;
}

CancellationSource::CancellationSource()
    : state_{std::make_shared<detail::CancellationState>()} {}

CancellationSource::~CancellationSource() {
  if (state_ != nullptr) {
    (void)state_->source_count.fetch_sub(1, std::memory_order_release);
  }
}

CancellationSource::CancellationSource(
    const CancellationSource& other) noexcept
    : state_{other.state_} {
  if (state_ != nullptr) {
    (void)state_->source_count.fetch_add(1, std::memory_order_relaxed);
  }
}

CancellationSource& CancellationSource::operator=(
    const CancellationSource& other) noexcept {
  if (state_ == other.state_) {
    return *this;
  }

  auto replacement = other.state_;
  if (replacement != nullptr) {
    (void)replacement->source_count.fetch_add(1, std::memory_order_relaxed);
  }
  if (state_ != nullptr) {
    (void)state_->source_count.fetch_sub(1, std::memory_order_release);
  }
  state_ = std::move(replacement);
  return *this;
}

CancellationSource::CancellationSource(CancellationSource&& other) noexcept
    : state_{std::move(other.state_)} {}

CancellationSource& CancellationSource::operator=(
    CancellationSource&& other) noexcept {
  if (this == &other) {
    return *this;
  }
  if (state_ != nullptr) {
    (void)state_->source_count.fetch_sub(1, std::memory_order_release);
  }
  state_ = std::move(other.state_);
  return *this;
}

CancellationToken CancellationSource::get_token() const noexcept {
  return CancellationToken{state_};
}

bool CancellationSource::stop_possible() const noexcept {
  return state_ != nullptr;
}

bool CancellationSource::stop_requested() const noexcept {
  return state_ != nullptr &&
         state_->stop_requested.load(std::memory_order_acquire);
}

bool CancellationSource::request_stop() noexcept {
  if (state_ == nullptr) {
    return false;
  }
  bool expected = false;
  return state_->stop_requested.compare_exchange_strong(
      expected, true, std::memory_order_acq_rel, std::memory_order_acquire);
}

bool operator==(const CancellationSource& first,
                const CancellationSource& second) noexcept {
  return first.state_ == second.state_;
}

}  // namespace ohl::media
