#include "ohl/media/payload_path.hpp"

#include <array>
#include <iostream>
#include <string>

namespace {

struct InvalidCase {
  std::string path;
  ohl::media::PayloadPathError error;
};

}  // namespace

int main() {
  const auto normalized =
      ohl::media::validate_payload_path(
          "ProjectFixture\\Nested/AmberToken.dat");
  if (!normalized.valid() ||
      normalized.relative_path != "ProjectFixture/Nested/AmberToken.dat" ||
      normalized.portability_key != "projectfixture/nested/ambertoken.dat") {
    std::cerr << "valid payload path was not normalized portably\n";
    return 1;
  }

  const std::array invalid_cases{
      InvalidCase{"", ohl::media::PayloadPathError::empty},
      InvalidCase{"/absolute", ohl::media::PayloadPathError::rooted},
      InvalidCase{"\\\\server\\share", ohl::media::PayloadPathError::rooted},
      InvalidCase{"C:\\payload", ohl::media::PayloadPathError::rooted},
      InvalidCase{"C:payload", ohl::media::PayloadPathError::rooted},
      InvalidCase{"one//two", ohl::media::PayloadPathError::empty_component},
      InvalidCase{"one/", ohl::media::PayloadPathError::empty_component},
      InvalidCase{"./one", ohl::media::PayloadPathError::traversal},
      InvalidCase{"one/../two", ohl::media::PayloadPathError::traversal},
      InvalidCase{"bad:name", ohl::media::PayloadPathError::invalid_character},
      InvalidCase{"bad\nname", ohl::media::PayloadPathError::invalid_character},
      InvalidCase{"trailing. ", ohl::media::PayloadPathError::invalid_character},
      InvalidCase{"caf\xc3\xa9", ohl::media::PayloadPathError::invalid_character},
      InvalidCase{"CON", ohl::media::PayloadPathError::reserved_name},
      InvalidCase{"nul.txt", ohl::media::PayloadPathError::reserved_name},
      InvalidCase{"Com9.cfg", ohl::media::PayloadPathError::reserved_name},
      InvalidCase{"com0", ohl::media::PayloadPathError::reserved_name},
      InvalidCase{"lpt1", ohl::media::PayloadPathError::reserved_name},
      InvalidCase{"LPT0.txt", ohl::media::PayloadPathError::reserved_name},
      InvalidCase{"conin$", ohl::media::PayloadPathError::reserved_name},
      InvalidCase{"ConOut$.log", ohl::media::PayloadPathError::reserved_name},
      InvalidCase{std::string(256, 'a'),
                  ohl::media::PayloadPathError::component_too_long},
      InvalidCase{std::string(4'097, 'a'),
                  ohl::media::PayloadPathError::too_long},
      InvalidCase{[] {
                    std::string path{"a"};
                    for (int index = 0; index < 32; ++index) {
                      path += "/a";
                    }
                    return path;
                  }(),
                  ohl::media::PayloadPathError::too_deep},
  };
  for (const auto& test : invalid_cases) {
    const auto result = ohl::media::validate_payload_path(test.path);
    if (result.error != test.error || !result.relative_path.empty() ||
        !result.portability_key.empty()) {
      std::cerr << "unsafe payload path was accepted or misclassified\n";
      return 1;
    }
  }

  const auto first =
      ohl::media::validate_payload_path("SyntheticBranch/CaseToken.DAT");
  const auto second =
      ohl::media::validate_payload_path("syntheticbranch/casetoken.dat");
  if (!first.valid() || !second.valid() ||
      first.portability_key != second.portability_key) {
    std::cerr << "case-insensitive collision key was not stable\n";
    return 1;
  }
  return 0;
}
