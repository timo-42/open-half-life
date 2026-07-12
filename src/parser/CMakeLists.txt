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

add_library(ohl_parser_worker_service STATIC
  src/parser_worker_service.cpp
)

add_library(
  OpenHalfLife::parser_worker_service
  ALIAS ohl_parser_worker_service
)

target_compile_features(ohl_parser_worker_service PRIVATE cxx_std_20)
target_link_libraries(
  ohl_parser_worker_service
  PRIVATE
    OpenHalfLife::parser
)
ohl_enable_warnings(ohl_parser_worker_service)
