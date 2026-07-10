#include "ohl/core/log.hpp"

#include <iostream>
#include <ostream>

namespace ohl::core {

Logger::Logger(std::ostream& output) noexcept : output_{output} {}

void Logger::set_minimum_level(const LogLevel level) noexcept {
  std::scoped_lock lock{mutex_};
  minimum_level_ = level;
}

LogLevel Logger::minimum_level() const noexcept {
  std::scoped_lock lock{mutex_};
  return minimum_level_;
}

void Logger::write(const LogLevel level, const std::string_view message) {
  std::scoped_lock lock{mutex_};
  if (level < minimum_level_) {
    return;
  }

  output_ << '[' << to_string(level) << "] " << message << '\n';
  output_.flush();
}

Logger& default_logger() {
  static Logger logger{std::clog};
  return logger;
}

void log(const LogLevel level, const std::string_view message) {
  default_logger().write(level, message);
}

}  // namespace ohl::core
