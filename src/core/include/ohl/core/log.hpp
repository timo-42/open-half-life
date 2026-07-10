#pragma once

#include <iosfwd>
#include <mutex>
#include <string_view>

namespace ohl::core {

enum class LogLevel {
  debug,
  info,
  warning,
  error,
};

[[nodiscard]] constexpr std::string_view to_string(LogLevel level) noexcept {
  switch (level) {
    case LogLevel::debug:
      return "debug";
    case LogLevel::info:
      return "info";
    case LogLevel::warning:
      return "warning";
    case LogLevel::error:
      return "error";
  }

  return "unknown";
}

class Logger final {
 public:
  explicit Logger(std::ostream& output) noexcept;

  void set_minimum_level(LogLevel level) noexcept;
  [[nodiscard]] LogLevel minimum_level() const noexcept;
  void write(LogLevel level, std::string_view message);

 private:
  std::ostream& output_;
  LogLevel minimum_level_{LogLevel::info};
  mutable std::mutex mutex_;
};

Logger& default_logger();
void log(LogLevel level, std::string_view message);

}  // namespace ohl::core
