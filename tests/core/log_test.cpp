#include "ohl/core/log.hpp"

#include <iostream>
#include <sstream>
#include <string>

int main() {
  std::ostringstream output;
  ohl::core::Logger logger{output};

  logger.write(ohl::core::LogLevel::debug, "hidden");
  logger.write(ohl::core::LogLevel::info, "ready");
  if (output.str() != "[info] ready\n") {
    std::cerr << "unexpected default logging output: " << output.str();
    return 1;
  }

  logger.set_minimum_level(ohl::core::LogLevel::debug);
  logger.write(ohl::core::LogLevel::debug, "visible");
  if (output.str() != "[info] ready\n[debug] visible\n") {
    std::cerr << "unexpected filtered logging output: " << output.str();
    return 1;
  }

  return 0;
}
