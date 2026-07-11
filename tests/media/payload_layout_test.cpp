#include "ohl/media/payload_layout.hpp"

#include <cstdint>
#include <iostream>
#include <string>
#include <vector>

namespace {

[[nodiscard]] bool rejects(
    std::vector<ohl::media::PayloadEntryMetadata> entries,
    const ohl::media::PayloadLayoutError expected,
    const ohl::media::PayloadImportLimits& limits = {}) {
  const auto result = ohl::media::plan_payload_layout(entries, limits);
  return result.error == expected && result.entries.empty();
}

}  // namespace

int main() {
  const std::vector entries{
      ohl::media::PayloadEntryMetadata{
          2, "ProjectFixture/Tiles/AmberBlob.dat", 2'048},
      ohl::media::PayloadEntryMetadata{
          3, "ProjectFixture\\Settings.note", 512},
      ohl::media::PayloadEntryMetadata{1, "AuthoredManifest.note", 0},
  };
  const auto planned = ohl::media::plan_payload_layout(entries);
  if (!planned.valid() || planned.entries.size() != entries.size() ||
      planned.total_bytes != 2'560 ||
      planned.entries[0].source_token != 1 ||
      planned.entries[1].relative_path !=
          "ProjectFixture/Settings.note" ||
      planned.rejected_entry.has_value()) {
    std::cerr << "valid payload layout was not planned\n";
    return 1;
  }

  if (!rejects({{1, "../escape", 1}},
               ohl::media::PayloadLayoutError::invalid_path) ||
      !rejects({{1, "SyntheticBranch/Alpha.item", 1},
                {2, "SYNTHETICBRANCH/ALPHA.ITEM", 1}},
               ohl::media::PayloadLayoutError::path_conflict) ||
      !rejects({{1, "FixtureRoot/alpha", 1},
                {2, "fixtureroot/beta", 1}},
               ohl::media::PayloadLayoutError::path_conflict) ||
      !rejects({{1, "FixtureLeaf", 1}, {2, "FixtureLeaf/child", 1}},
               ohl::media::PayloadLayoutError::path_conflict) ||
      !rejects({{1, "FixtureTree/child", 1}, {2, "FIXTURETREE", 1}},
               ohl::media::PayloadLayoutError::path_conflict)) {
    std::cerr << "unsafe or ambiguous payload layout was accepted\n";
    return 1;
  }

  const ohl::media::PayloadImportLimits small_limits{
      .maximum_entries = 2,
      .maximum_path_bytes = 10,
      .maximum_entry_bytes = 10,
      .maximum_total_bytes = 15,
  };
  if (!rejects({{1, "a", 1}, {2, "b", 1}, {3, "c", 1}},
               ohl::media::PayloadLayoutError::too_many_entries,
               small_limits) ||
      !rejects({{1, "123456", 1}, {2, "12345", 1}},
               ohl::media::PayloadLayoutError::too_many_path_bytes,
               small_limits) ||
      !rejects({{1, "a", 1}, {1, "b", 1}},
               ohl::media::PayloadLayoutError::source_token_conflict,
               small_limits) ||
      !rejects({{1, "a", 11}},
               ohl::media::PayloadLayoutError::entry_too_large,
               small_limits) ||
      !rejects({{1, "a", 8}, {2, "b", 8}},
               ohl::media::PayloadLayoutError::payload_too_large,
               small_limits)) {
    std::cerr << "payload resource limits were not enforced\n";
    return 1;
  }

  const ohl::media::PayloadImportLimits invalid_limits{
      .maximum_entries = 1,
      .maximum_path_bytes = 1,
      .maximum_entry_bytes = 2,
      .maximum_total_bytes = 1,
  };
  if (!rejects({}, ohl::media::PayloadLayoutError::invalid_limits,
               invalid_limits)) {
    std::cerr << "invalid payload limits were accepted\n";
    return 1;
  }
  return 0;
}
