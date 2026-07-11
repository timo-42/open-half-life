#pragma once

#include <memory>

namespace ohl::media {
namespace detail {

class CancellationState;

}  // namespace detail

class CancellationSource;

// A cheap, copyable cancellation observation handle. A default token has no
// state, cannot be stopped, and never reports cancellation. Tokens retain their
// shared state after every source referring to it has been destroyed. An
// unstopped token then reports that stopping is no longer possible; an already
// stopped token continues to report both possible and requested.
class CancellationToken final {
 public:
  CancellationToken() noexcept = default;

  [[nodiscard]] bool stop_possible() const noexcept;
  [[nodiscard]] bool stop_requested() const noexcept;

  friend bool operator==(const CancellationToken& first,
                         const CancellationToken& second) noexcept;

 private:
  friend class CancellationSource;

  explicit CancellationToken(
      std::shared_ptr<detail::CancellationState> state) noexcept;

  std::shared_ptr<detail::CancellationState> state_;
};

// Owns a shared cancellation state. Copies refer to the same state. A moved-
// from source has no state. request_stop() returns true only for the first
// request made through any source sharing that state.
class CancellationSource final {
 public:
  CancellationSource();
  ~CancellationSource();

  CancellationSource(const CancellationSource& other) noexcept;
  CancellationSource& operator=(const CancellationSource& other) noexcept;
  CancellationSource(CancellationSource&& other) noexcept;
  CancellationSource& operator=(CancellationSource&& other) noexcept;

  [[nodiscard]] CancellationToken get_token() const noexcept;
  [[nodiscard]] bool stop_possible() const noexcept;
  [[nodiscard]] bool stop_requested() const noexcept;
  [[nodiscard]] bool request_stop() noexcept;

  friend bool operator==(const CancellationSource& first,
                         const CancellationSource& second) noexcept;

 private:
  std::shared_ptr<detail::CancellationState> state_;
};

}  // namespace ohl::media
