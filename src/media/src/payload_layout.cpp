#include "ohl/media/payload_layout.hpp"

#include "ohl/media/payload_path.hpp"

#include <algorithm>
#include <unordered_map>
#include <unordered_set>
#include <utility>

namespace ohl::media {

PayloadLayout plan_payload_layout(
    const std::vector<PayloadEntryMetadata>& entries,
    const PayloadImportLimits& limits) {
  PayloadLayout result;
  if (limits.maximum_entries == 0 || limits.maximum_path_bytes == 0 ||
      limits.maximum_entry_bytes == 0 || limits.maximum_total_bytes == 0 ||
      limits.maximum_entry_bytes > limits.maximum_total_bytes) {
    result.error = PayloadLayoutError::invalid_limits;
    return result;
  }
  if (entries.size() > limits.maximum_entries) {
    result.error = PayloadLayoutError::too_many_entries;
    return result;
  }

  std::unordered_set<std::uint64_t> source_tokens;
  std::unordered_set<std::string> file_keys;
  std::unordered_map<std::string, std::string> directory_spellings;
  std::vector<std::pair<std::string, PlannedPayloadEntry>> planned_entries;
  source_tokens.reserve(entries.size());
  file_keys.reserve(entries.size());
  directory_spellings.reserve(entries.size());
  planned_entries.reserve(entries.size());

  std::uint64_t total_bytes = 0;
  std::uint64_t total_path_bytes = 0;

  for (std::size_t index = 0; index < entries.size(); ++index) {
    const auto& source = entries[index];
    result.rejected_entry = index;
    if (!source_tokens.insert(source.source_token).second) {
      result.error = PayloadLayoutError::source_token_conflict;
      return result;
    }
    const auto path_size = static_cast<std::uint64_t>(source.archive_path.size());
    if (path_size > limits.maximum_path_bytes - total_path_bytes) {
      result.error = PayloadLayoutError::too_many_path_bytes;
      return result;
    }
    if (source.size_bytes > limits.maximum_entry_bytes) {
      result.error = PayloadLayoutError::entry_too_large;
      return result;
    }
    if (source.size_bytes > limits.maximum_total_bytes - total_bytes) {
      result.error = PayloadLayoutError::payload_too_large;
      return result;
    }

    auto path = validate_payload_path(source.archive_path);
    if (!path.valid()) {
      result.error = PayloadLayoutError::invalid_path;
      result.path_error = path.error;
      return result;
    }
    if (file_keys.contains(path.portability_key) ||
        directory_spellings.contains(path.portability_key)) {
      result.error = PayloadLayoutError::path_conflict;
      return result;
    }
    for (auto separator = path.portability_key.find('/');
         separator != std::string::npos;
         separator = path.portability_key.find('/', separator + 1)) {
      const auto parent_key = path.portability_key.substr(0, separator);
      if (file_keys.contains(parent_key)) {
        result.error = PayloadLayoutError::path_conflict;
        return result;
      }
      const auto parent_spelling = path.relative_path.substr(0, separator);
      const auto [existing, inserted] =
          directory_spellings.emplace(parent_key, parent_spelling);
      if (!inserted && existing->second != parent_spelling) {
        result.error = PayloadLayoutError::path_conflict;
        return result;
      }
    }
    auto key = path.portability_key;
    file_keys.insert(key);
    total_bytes += source.size_bytes;
    total_path_bytes += path_size;
    planned_entries.emplace_back(
        std::move(key), PlannedPayloadEntry{
                            .source_token = source.source_token,
                            .relative_path = std::move(path.relative_path),
                            .size_bytes = source.size_bytes,
                        });
  }
  std::ranges::sort(planned_entries, {}, [](const auto& item) -> const auto& {
    return item.first;
  });
  result.entries.reserve(planned_entries.size());
  for (auto& item : planned_entries) {
    result.entries.push_back(std::move(item.second));
  }
  result.total_bytes = total_bytes;
  result.rejected_entry.reset();
  return result;
}

}  // namespace ohl::media
