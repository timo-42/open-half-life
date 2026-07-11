add_library(ohl_parser STATIC
  src/protocol.cpp
  src/protocol_messages.cpp
)

add_library(OpenHalfLife::parser ALIAS ohl_parser)

target_compile_features(ohl_parser PUBLIC cxx_std_20)
target_include_directories(
  ohl_parser
  PUBLIC
    $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/include>
)
ohl_enable_warnings(ohl_parser)
