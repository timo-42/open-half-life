#pragma once

#include "ohl/media/iso_inspector.hpp"

#include <stop_token>

namespace ohl::media::detail {

enum class SourceStabilityError {
  none,
  invalid_capability,
  source_changed,
  read_failure,
  digest_mismatch,
  cancelled,
};

// Reauthenticates the complete content of an already validated, pinned source.
// Results intentionally carry no source identity, bytes, or digest material.
[[nodiscard]] SourceStabilityError verify_complete_source_stability(
    const ValidatedMedia& media, std::stop_token stop_token = {});

}  // namespace ohl::media::detail
