#pragma once

#include "ohl/media/payload_path.hpp"

#include <cstddef>
#include <cstdint>
#include <optional>
#include <string>
#include <vector>

namespace ohl::media {

struct PayloadEntryMetadata {
  std::uint64_t source_token{0};
  std::string archive_path;
  std::uint64_t size_bytes{0};
};

struct PayloadImportLimits {
  std::size_t maximum_entries{50'000};
  std::uint64_t maximum_path_bytes{64ULL * 1'024 * 1'024};
  std::uint64_t maximum_entry_bytes{8ULL * 1'024 * 1'024 * 1'024};
  std::uint64_t maximum_total_bytes{32ULL * 1'024 * 1'024 * 1'024};
};

enum class PayloadLayoutError {
  none,
  invalid_limits,
  too_many_entries,
  too_many_path_bytes,
  source_token_conflict,
  invalid_path,
  path_conflict,
  entry_too_large,
  payload_too_large,
};

struct PlannedPayloadEntry {
  std::uint64_t source_token{0};
  std::string relative_path;
  std::uint64_t size_bytes{0};
};

struct PayloadLayout {
  PayloadLayoutError error{PayloadLayoutError::none};
  PayloadPathError path_error{PayloadPathError::none};
  std::optional<std::size_t> rejected_entry;
  std::uint64_t total_bytes{0};
  std::vector<PlannedPayloadEntry> entries;

  [[nodiscard]] bool valid() const noexcept {
    return error == PayloadLayoutError::none;
  }
};

// Validates all archive-controlled metadata before any destination is opened.
// Conflicts include exact duplicates, ASCII case-only aliases, and file versus
// directory prefix aliases.
[[nodiscard]] PayloadLayout plan_payload_layout(
    const std::vector<PayloadEntryMetadata>& entries,
    const PayloadImportLimits& limits = {});

}  // namespace ohl::media
