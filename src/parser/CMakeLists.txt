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

if(CMAKE_SYSTEM_NAME STREQUAL "Linux")
  string(TOLOWER "${CMAKE_SYSTEM_PROCESSOR}" ohl_parser_worker_processor)
  if(ohl_parser_worker_processor MATCHES "^(x86_64|amd64)$")
    # Private, freestanding copy of the protocol and service used only by the
    # installed Linux worker. It is deliberately not exported or installed.
    add_library(ohl_parser_worker_runtime STATIC
      src/protocol.cpp
      src/protocol_messages.cpp
      src/parser_worker_service.cpp
    )
    target_compile_features(ohl_parser_worker_runtime PRIVATE cxx_std_20)
    target_include_directories(
      ohl_parser_worker_runtime
      PRIVATE
        ${CMAKE_CURRENT_SOURCE_DIR}/include
        ${CMAKE_CURRENT_SOURCE_DIR}/src
    )
    target_compile_options(
      ohl_parser_worker_runtime
      PRIVATE
        -ffreestanding
        -fno-exceptions
        -fno-rtti
        -fno-stack-protector
        -fno-pie
        -fno-sanitize=all
    )
    ohl_enable_warnings(ohl_parser_worker_runtime)
  endif()
endif()
