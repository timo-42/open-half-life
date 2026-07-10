#pragma once

#include <string>
#include <string_view>

namespace ohl::media {

enum class PayloadPathError {
  none,
  empty,
  too_long,
  rooted,
  empty_component,
  traversal,
  too_deep,
  invalid_character,
  component_too_long,
  reserved_name,
};

struct PayloadPath {
  PayloadPathError error{PayloadPathError::none};
  // Slash-separated, relative, and lexically portable across supported hosts.
  // Extraction must still use no-follow, create-new native filesystem calls.
  std::string relative_path;
  // ASCII case-folded key for rejecting collisions before extraction.
  std::string portability_key;

  [[nodiscard]] bool valid() const noexcept {
    return error == PayloadPathError::none;
  }
};

// Applies a deliberately strict common-denominator filename policy for
// imported payloads. Backslashes are normalized to slashes. Only printable
// ASCII is accepted until Unicode normalization and case folding are defined.
[[nodiscard]] PayloadPath validate_payload_path(std::string_view path);

}  // namespace ohl::media
