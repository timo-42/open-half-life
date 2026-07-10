#include "ohl/media/payload_path.hpp"

#include <algorithm>

namespace ohl::media {
namespace {

constexpr std::size_t kMaximumPathLength = 4'096;
constexpr std::size_t kMaximumComponentLength = 255;
constexpr std::size_t kMaximumComponentCount = 32;

[[nodiscard]] constexpr char ascii_upper(const char character) noexcept {
  return character >= 'a' && character <= 'z'
             ? static_cast<char>(character - ('a' - 'A'))
             : character;
}

[[nodiscard]] constexpr char ascii_lower(const char character) noexcept {
  return character >= 'A' && character <= 'Z'
             ? static_cast<char>(character + ('a' - 'A'))
             : character;
}

[[nodiscard]] bool is_reserved_windows_name(
    const std::string_view component) {
  const auto extension = component.find('.');
  auto base = std::string{component.substr(0, extension)};
  std::transform(base.begin(), base.end(), base.begin(), ascii_upper);
  if (base == "CON" || base == "PRN" || base == "AUX" || base == "NUL" ||
      base == "CLOCK$" || base == "CONIN$" || base == "CONOUT$") {
    return true;
  }
  return base.size() == 4 &&
         (base.starts_with("COM") || base.starts_with("LPT")) &&
         base[3] >= '0' && base[3] <= '9';
}

[[nodiscard]] PayloadPathError validate_component(
    const std::string_view component) {
  if (component.empty()) {
    return PayloadPathError::empty_component;
  }
  if (component == "." || component == "..") {
    return PayloadPathError::traversal;
  }
  if (component.size() > kMaximumComponentLength) {
    return PayloadPathError::component_too_long;
  }
  if (component.ends_with('.') || component.ends_with(' ')) {
    return PayloadPathError::invalid_character;
  }
  for (const auto character : component) {
    const auto byte = static_cast<unsigned char>(character);
    if (byte < 0x20U || byte > 0x7eU ||
        std::string_view{"<>:\"|?*"}.find(character) !=
            std::string_view::npos) {
      return PayloadPathError::invalid_character;
    }
  }
  if (is_reserved_windows_name(component)) {
    return PayloadPathError::reserved_name;
  }
  return PayloadPathError::none;
}

}  // namespace

PayloadPath validate_payload_path(const std::string_view path) {
  PayloadPath result;
  if (path.empty()) {
    result.error = PayloadPathError::empty;
    return result;
  }
  if (path.size() > kMaximumPathLength) {
    result.error = PayloadPathError::too_long;
    return result;
  }
  if (path.front() == '/' || path.front() == '\\' ||
      (path.size() >= 2 && path[1] == ':')) {
    result.error = PayloadPathError::rooted;
    return result;
  }

  std::size_t component_start = 0;
  std::size_t component_count = 0;
  while (component_start <= path.size()) {
    ++component_count;
    if (component_count > kMaximumComponentCount) {
      result.error = PayloadPathError::too_deep;
      result.relative_path.clear();
      result.portability_key.clear();
      return result;
    }
    const auto component_end = path.find_first_of("/\\", component_start);
    const auto component = path.substr(
        component_start,
        component_end == std::string_view::npos
            ? std::string_view::npos
            : component_end - component_start);
    result.error = validate_component(component);
    if (result.error != PayloadPathError::none) {
      result.relative_path.clear();
      result.portability_key.clear();
      return result;
    }

    if (!result.relative_path.empty()) {
      result.relative_path.push_back('/');
      result.portability_key.push_back('/');
    }
    result.relative_path.append(component);
    for (const auto character : component) {
      result.portability_key.push_back(ascii_lower(character));
    }

    if (component_end == std::string_view::npos) {
      break;
    }
    component_start = component_end + 1;
  }
  return result;
}

}  // namespace ohl::media
