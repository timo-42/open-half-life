option(
  OHL_REQUIRE_LINUX_ISOLATED_WORKER
  "Fail configuration when the Linux x64 isolated-worker backend is unavailable"
  OFF
)

string(TOLOWER "${CMAKE_SYSTEM_PROCESSOR}" ohl_linux_worker_processor)
if(ohl_linux_worker_processor MATCHES "^(x86_64|amd64)$")
  target_sources(ohl_platform PRIVATE src/isolated_worker_linux.cpp)
  add_executable(
    ohl_media_parser_worker
    src/media_parser_worker_linux.cpp
  )
  target_compile_features(ohl_media_parser_worker PRIVATE cxx_std_20)
  target_include_directories(
    ohl_media_parser_worker
    PRIVATE
      src
      ${PROJECT_SOURCE_DIR}/src/parser/include
      ${PROJECT_SOURCE_DIR}/src/parser/src
  )
  target_compile_definitions(
    ohl_media_parser_worker
    PRIVATE OHL_LINUX_ISOLATED_WORKER_FREESTANDING=1
  )
  target_compile_options(
    ohl_media_parser_worker
    PRIVATE
      -ffreestanding
      -fno-exceptions
      -fno-rtti
      -fno-stack-protector
      -fno-pie
      -fno-sanitize=all
  )
  target_link_options(
    ohl_media_parser_worker PRIVATE -nostdlib -static -no-pie -Wl,-e,_start
  )
  target_link_libraries(
    ohl_media_parser_worker PRIVATE ohl_parser_worker_runtime
  )
  set_target_properties(
    ohl_media_parser_worker
    PROPERTIES OUTPUT_NAME ohl-media-parser-worker
  )
  ohl_enable_warnings(ohl_media_parser_worker)
  install(
    TARGETS ohl_media_parser_worker
    RUNTIME
      DESTINATION libexec/open-half-life
      COMPONENT media_parser_worker
      PERMISSIONS
        OWNER_READ OWNER_EXECUTE
        GROUP_READ GROUP_EXECUTE
        WORLD_READ WORLD_EXECUTE
  )
elseif(OHL_REQUIRE_LINUX_ISOLATED_WORKER)
  message(
    FATAL_ERROR
    "The required Linux isolated-worker backend supports x86-64 only; "
    "CMAKE_SYSTEM_PROCESSOR is '${CMAKE_SYSTEM_PROCESSOR}'"
  )
else()
  target_sources(ohl_platform PRIVATE src/isolated_worker_unsupported.cpp)
endif()
