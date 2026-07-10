#include "ohl/core/sha256.hpp"

#include <cstddef>
#include <iostream>
#include <span>
#include <string>
#include <string_view>

namespace {

int expect_digest(const std::string_view input,
                  const std::string_view expected) {
  ohl::core::Sha256 sha256;
  const auto bytes = std::as_bytes(std::span{input});
  sha256.update(bytes.first(bytes.size() / 2));
  sha256.update(bytes.subspan(bytes.size() / 2));
  const auto actual = ohl::core::hex_encode(sha256.finish());
  if (actual != expected || ohl::core::hex_encode(sha256.finish()) != expected) {
    std::cerr << "SHA-256 digest mismatch for a " << input.size()
              << "-byte input\n";
    return 1;
  }
  return 0;
}

}  // namespace

int main() {
  if (expect_digest(
          "", "e3b0c44298fc1c149afbf4c8996fb924"
              "27ae41e4649b934ca495991b7852b855") != 0 ||
      expect_digest(
          "abc", "ba7816bf8f01cfea414140de5dae2223"
                 "b00361a396177a9cb410ff61f20015ad") != 0 ||
      expect_digest(
          std::string(1'000, 'a'),
          "41edece42d63e8d9bf515a9ba6932e1c"
          "20cbc9f5a5d134645adb5db1b9737ea3") != 0) {
    return 1;
  }
  return 0;
}
