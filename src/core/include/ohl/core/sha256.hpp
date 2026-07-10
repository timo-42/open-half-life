#pragma once

#include <array>
#include <cstddef>
#include <cstdint>
#include <span>
#include <string>

namespace ohl::core {

class Sha256 final {
 public:
  Sha256() noexcept;

  void update(std::span<const std::byte> bytes) noexcept;
  [[nodiscard]] std::array<std::byte, 32> finish() noexcept;

 private:
  void transform(std::span<const std::byte, 64> block) noexcept;

  std::array<std::uint32_t, 8> state_{};
  std::array<std::byte, 64> buffer_{};
  std::array<std::byte, 32> digest_{};
  std::uint64_t total_bytes_{0};
  std::size_t buffered_bytes_{0};
  bool finished_{false};
};

[[nodiscard]] std::string hex_encode(std::span<const std::byte> bytes);

}  // namespace ohl::core
