#include "isolated_worker_internal.hpp"

namespace ohl::platform::detail {

IsolatedWorkerBackendLaunchResult launch_isolated_worker_backend(
    const IsolatedWorkerService,
    const std::chrono::steady_clock::time_point) noexcept {
  return {.backend = nullptr, .error = IsolatedWorkerError::unsupported};
}

}  // namespace ohl::platform::detail
