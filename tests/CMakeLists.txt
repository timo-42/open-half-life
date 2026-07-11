add_executable(ohl_core_tests
  core/log_test.cpp
)
target_compile_features(ohl_core_tests PRIVATE cxx_std_20)
target_link_libraries(ohl_core_tests PRIVATE OpenHalfLife::core)
ohl_enable_warnings(ohl_core_tests)
add_test(NAME core.logging COMMAND ohl_core_tests)

add_executable(ohl_sha256_tests
  core/sha256_test.cpp
)
target_compile_features(ohl_sha256_tests PRIVATE cxx_std_20)
target_link_libraries(ohl_sha256_tests PRIVATE OpenHalfLife::core)
ohl_enable_warnings(ohl_sha256_tests)
add_test(NAME core.sha256 COMMAND ohl_sha256_tests)

add_executable(ohl_parser_tests
  parser/protocol_test.cpp
)
target_compile_features(ohl_parser_tests PRIVATE cxx_std_20)
target_link_libraries(ohl_parser_tests PRIVATE OpenHalfLife::parser)
ohl_enable_warnings(ohl_parser_tests)
add_test(NAME parser.protocol COMMAND ohl_parser_tests)

add_executable(ohl_media_parser_result_tests
  media/parser_result_session_test.cpp
)
target_compile_features(ohl_media_parser_result_tests PRIVATE cxx_std_20)
target_link_libraries(
  ohl_media_parser_result_tests
  PRIVATE OpenHalfLife::media_parser_results
)
ohl_enable_warnings(ohl_media_parser_result_tests)
add_test(
  NAME media.parser_result_session
  COMMAND ohl_media_parser_result_tests
)

add_executable(ohl_media_parser_source_read_broker_tests
  media/parser_source_read_broker_test.cpp
)
target_compile_features(
  ohl_media_parser_source_read_broker_tests PRIVATE cxx_std_20
)
target_link_libraries(
  ohl_media_parser_source_read_broker_tests
  PRIVATE OpenHalfLife::media_parser_reads
)
ohl_enable_warnings(ohl_media_parser_source_read_broker_tests)
add_test(
  NAME media.parser_source_read_broker
  COMMAND ohl_media_parser_source_read_broker_tests
)

add_library(ohl_parser_fuzz_entry OBJECT
  parser/protocol_fuzz.cpp
)
target_compile_features(ohl_parser_fuzz_entry PRIVATE cxx_std_20)
target_link_libraries(ohl_parser_fuzz_entry PRIVATE OpenHalfLife::parser)
ohl_enable_warnings(ohl_parser_fuzz_entry)

if(OHL_BUILD_FUZZERS)
  include(CheckCXXSourceCompiles)

  function(ohl_find_libfuzzer_flag output_variable)
    set(ohl_libfuzzer_probe [=[
      #include <cstddef>
      #include <cstdint>

      extern "C" int LLVMFuzzerTestOneInput(
          const std::uint8_t*, std::size_t) {
        return 0;
      }
    ]=])
    set(CMAKE_TRY_COMPILE_TARGET_TYPE EXECUTABLE)

    set(CMAKE_REQUIRED_FLAGS "-fsanitize=fuzzer")
    set(CMAKE_REQUIRED_LINK_OPTIONS "-fsanitize=fuzzer")
    check_cxx_source_compiles(
      "${ohl_libfuzzer_probe}"
      OHL_LIBFUZZER_DASH_FLAG_LINKS
    )
    if(OHL_LIBFUZZER_DASH_FLAG_LINKS)
      set(${output_variable} "-fsanitize=fuzzer" PARENT_SCOPE)
      return()
    endif()

    set(CMAKE_REQUIRED_FLAGS "/fsanitize=fuzzer")
    set(CMAKE_REQUIRED_LINK_OPTIONS "/fsanitize=fuzzer")
    check_cxx_source_compiles(
      "${ohl_libfuzzer_probe}"
      OHL_LIBFUZZER_SLASH_FLAG_LINKS
    )
    if(OHL_LIBFUZZER_SLASH_FLAG_LINKS)
      set(${output_variable} "/fsanitize=fuzzer" PARENT_SCOPE)
      return()
    endif()

    set(${output_variable} "" PARENT_SCOPE)
  endfunction()

  ohl_find_libfuzzer_flag(ohl_libfuzzer_flag)
  if(ohl_libfuzzer_flag STREQUAL "")
    message(
      FATAL_ERROR
      "OHL_BUILD_FUZZERS=ON requires a compiler and runtime that can "
      "compile and link libFuzzer. Neither -fsanitize=fuzzer nor "
      "/fsanitize=fuzzer linked an LLVMFuzzerTestOneInput probe."
    )
  endif()

  add_library(ohl_parser_fuzzer_parser STATIC
    ${PROJECT_SOURCE_DIR}/src/parser/src/protocol.cpp
    ${PROJECT_SOURCE_DIR}/src/parser/src/protocol_messages.cpp
  )
  target_compile_features(ohl_parser_fuzzer_parser PUBLIC cxx_std_20)
  target_include_directories(
    ohl_parser_fuzzer_parser
    PUBLIC ${PROJECT_SOURCE_DIR}/src/parser/include
  )
  target_compile_options(
    ohl_parser_fuzzer_parser PRIVATE ${ohl_libfuzzer_flag}
  )
  ohl_enable_warnings(ohl_parser_fuzzer_parser)

  add_executable(ohl_parser_protocol_fuzzer
    parser/protocol_fuzz.cpp
  )
  target_compile_features(ohl_parser_protocol_fuzzer PRIVATE cxx_std_20)
  target_link_libraries(
    ohl_parser_protocol_fuzzer PRIVATE ohl_parser_fuzzer_parser
  )
  target_compile_options(
    ohl_parser_protocol_fuzzer PRIVATE ${ohl_libfuzzer_flag}
  )
  target_link_options(
    ohl_parser_protocol_fuzzer PRIVATE ${ohl_libfuzzer_flag}
  )
  ohl_enable_warnings(ohl_parser_protocol_fuzzer)

  set(
    ohl_parser_fuzz_corpus
    enumerate_complete.txt
    enumerate_crossed_cancel.txt
    stream_cancel.txt
    stream_crossed_complete.txt
  )
  set(
    ohl_parser_fuzz_corpus_dir
    ${CMAKE_CURRENT_BINARY_DIR}/parser_protocol_fuzz_corpus
  )
  file(MAKE_DIRECTORY "${ohl_parser_fuzz_corpus_dir}")
  set(ohl_parser_fuzz_seed_paths)
  foreach(seed IN LISTS ohl_parser_fuzz_corpus)
    set(
      ohl_parser_fuzz_seed_path
      "${ohl_parser_fuzz_corpus_dir}/${seed}"
    )
    configure_file(
      "${CMAKE_CURRENT_SOURCE_DIR}/parser/corpus/${seed}"
      "${ohl_parser_fuzz_seed_path}"
      COPYONLY
    )
    list(APPEND ohl_parser_fuzz_seed_paths "${ohl_parser_fuzz_seed_path}")
  endforeach()
  add_test(
    NAME parser_protocol_fuzz_smoke
    COMMAND
      ohl_parser_protocol_fuzzer
      -seed=1
      -max_len=8192
      ${ohl_parser_fuzz_seed_paths}
  )
  set_tests_properties(parser_protocol_fuzz_smoke PROPERTIES TIMEOUT 30)
endif()

add_executable(ohl_platform_tests
  platform/platform_test.cpp
)
target_compile_features(ohl_platform_tests PRIVATE cxx_std_20)
target_link_libraries(ohl_platform_tests PRIVATE OpenHalfLife::platform)
ohl_enable_warnings(ohl_platform_tests)
add_test(NAME platform.detection COMMAND ohl_platform_tests)
if(DEFINED OHL_EXPECTED_OS AND NOT OHL_EXPECTED_OS STREQUAL "")
  target_compile_definitions(
    ohl_platform_tests
    PRIVATE
      OHL_EXPECTED_OS="${OHL_EXPECTED_OS}"
      OHL_EXPECTED_ARCH="${OHL_EXPECTED_ARCH}"
  )
endif()

add_executable(ohl_isolated_worker_common_tests
  platform/isolated_worker_common_test.cpp
  ${PROJECT_SOURCE_DIR}/src/platform/src/isolated_worker.cpp
)
target_compile_features(ohl_isolated_worker_common_tests PRIVATE cxx_std_20)
target_include_directories(
  ohl_isolated_worker_common_tests
  PRIVATE
    ${PROJECT_SOURCE_DIR}/src/platform/include
    ${PROJECT_SOURCE_DIR}/src/platform/src
)
target_link_libraries(
  ohl_isolated_worker_common_tests PRIVATE Threads::Threads
)
ohl_enable_warnings(ohl_isolated_worker_common_tests)
add_test(
  NAME platform.isolated_worker.common
  COMMAND ohl_isolated_worker_common_tests
)
set_tests_properties(platform.isolated_worker.common PROPERTIES TIMEOUT 30)

add_executable(ohl_media_source_tests
  platform/media_source_test.cpp
)
target_compile_features(ohl_media_source_tests PRIVATE cxx_std_20)
target_link_libraries(
  ohl_media_source_tests
  PRIVATE
    OpenHalfLife::platform
    Threads::Threads
)
if(CMAKE_SYSTEM_NAME STREQUAL "Linux")
  find_library(OHL_TEST_UTIL_LIBRARY util REQUIRED)
  target_link_libraries(ohl_media_source_tests PRIVATE ${OHL_TEST_UTIL_LIBRARY})
endif()
ohl_enable_warnings(ohl_media_source_tests)
add_test(NAME platform.media_source COMMAND ohl_media_source_tests)
set_tests_properties(platform.media_source PROPERTIES TIMEOUT 30)

if(CMAKE_SYSTEM_NAME STREQUAL "Linux" OR
   CMAKE_SYSTEM_NAME STREQUAL "Darwin" OR
   CMAKE_SYSTEM_NAME STREQUAL "Windows")
  add_executable(ohl_media_source_native_security_tests
    platform/media_source_native_security_test.cpp
  )
  target_compile_features(
    ohl_media_source_native_security_tests PRIVATE cxx_std_20
  )
  target_include_directories(
    ohl_media_source_native_security_tests
    PRIVATE ${PROJECT_SOURCE_DIR}/src/platform/include
  )
  ohl_enable_warnings(ohl_media_source_native_security_tests)
  add_test(
    NAME platform.media_source_native_security
    COMMAND ohl_media_source_native_security_tests
  )
  set_tests_properties(
    platform.media_source_native_security PROPERTIES TIMEOUT 30
  )
endif()

add_executable(ohl_media_source_unsupported_tests
  platform/media_source_test.cpp
  ${PROJECT_SOURCE_DIR}/src/platform/src/media_source_unsupported.cpp
)
target_compile_features(ohl_media_source_unsupported_tests PRIVATE cxx_std_20)
target_compile_definitions(
  ohl_media_source_unsupported_tests
  PRIVATE OHL_PLATFORM_MEDIA_SOURCE_UNSUPPORTED=1
)
target_include_directories(
  ohl_media_source_unsupported_tests
  PRIVATE ${PROJECT_SOURCE_DIR}/src/platform/include
)
ohl_enable_warnings(ohl_media_source_unsupported_tests)
add_test(
  NAME platform.media_source_unsupported
  COMMAND ohl_media_source_unsupported_tests
)
set_tests_properties(platform.media_source_unsupported PROPERTIES TIMEOUT 30)

if(CMAKE_SYSTEM_NAME STREQUAL "Linux")
  option(
    OHL_REQUIRE_LINUX_ATOMIC_STORE_CAPABILITIES
    "Fail rather than skip when Linux atomic-store capabilities are absent"
    OFF
  )
  add_executable(ohl_atomic_directory_store_linux_tests
    platform/atomic_directory_store_linux_test.cpp
  )
  target_compile_features(
    ohl_atomic_directory_store_linux_tests PRIVATE cxx_std_20
  )
  target_include_directories(
    ohl_atomic_directory_store_linux_tests
    PRIVATE ${PROJECT_SOURCE_DIR}/src/platform/src
  )
  target_link_libraries(
    ohl_atomic_directory_store_linux_tests PRIVATE OpenHalfLife::platform
  )
  target_compile_definitions(
    ohl_atomic_directory_store_linux_tests
    PRIVATE
      OHL_REQUIRE_LINUX_ATOMIC_STORE_CAPABILITIES=$<BOOL:${OHL_REQUIRE_LINUX_ATOMIC_STORE_CAPABILITIES}>
  )
  ohl_enable_warnings(ohl_atomic_directory_store_linux_tests)
  foreach(native_case
      root_security
      component_io
      marker_last
      probe_exact_tree
      cleanup
      publish_no_replace
      race_matching
      race_conflicting
      fault_injection)
    add_test(
      NAME platform.atomic_directory_store.linux.${native_case}
      COMMAND ohl_atomic_directory_store_linux_tests ${native_case}
    )
    set_tests_properties(
      platform.atomic_directory_store.linux.${native_case}
      PROPERTIES
        SKIP_RETURN_CODE 77
        TIMEOUT 20
        LABELS linux-native
    )
  endforeach()
  set_tests_properties(
    platform.atomic_directory_store.linux.race_matching
    platform.atomic_directory_store.linux.race_conflicting
    PROPERTIES LABELS "linux-native;linux-race" TIMEOUT 30
  )

  add_executable(ohl_payload_stage_linux_native_tests
    media/payload_stage_linux_test.cpp
  )
  target_compile_features(
    ohl_payload_stage_linux_native_tests PRIVATE cxx_std_20
  )
  target_link_libraries(
    ohl_payload_stage_linux_native_tests PRIVATE OpenHalfLife::media
  )
  target_compile_definitions(
    ohl_payload_stage_linux_native_tests
    PRIVATE
      OHL_REQUIRE_LINUX_ATOMIC_STORE_CAPABILITIES=$<BOOL:${OHL_REQUIRE_LINUX_ATOMIC_STORE_CAPABILITIES}>
  )
  ohl_enable_warnings(ohl_payload_stage_linux_native_tests)
  add_test(
    NAME media.payload_stage.linux_native
    COMMAND ohl_payload_stage_linux_native_tests
  )
  set_tests_properties(
    media.payload_stage.linux_native
    PROPERTIES
      SKIP_RETURN_CODE 77
      TIMEOUT 20
      LABELS linux-native
  )
endif()

add_executable(ohl_media_tests
  media/iso_inspector_test.cpp
)
target_compile_features(ohl_media_tests PRIVATE cxx_std_20)
target_link_libraries(ohl_media_tests PRIVATE OpenHalfLife::media)
ohl_enable_warnings(ohl_media_tests)
add_test(NAME media.iso_inspector COMMAND ohl_media_tests)

add_executable(ohl_media_cancellation_tests
  media/cancellation_test.cpp
)
target_compile_features(ohl_media_cancellation_tests PRIVATE cxx_std_20)
target_link_libraries(
  ohl_media_cancellation_tests
  PRIVATE
    OpenHalfLife::media
    Threads::Threads
)
ohl_enable_warnings(ohl_media_cancellation_tests)
add_test(NAME media.cancellation COMMAND ohl_media_cancellation_tests)
set_tests_properties(media.cancellation PROPERTIES TIMEOUT 30)

add_executable(ohl_import_cache_tests
  media/import_cache_test.cpp
)
target_compile_features(ohl_import_cache_tests PRIVATE cxx_std_20)
target_include_directories(
  ohl_import_cache_tests
  PRIVATE ${PROJECT_SOURCE_DIR}/src/media/src
)
target_link_libraries(ohl_import_cache_tests PRIVATE OpenHalfLife::media)
ohl_enable_warnings(ohl_import_cache_tests)
add_test(NAME media.import_cache COMMAND ohl_import_cache_tests)

add_executable(ohl_payload_path_tests
  media/payload_path_test.cpp
)
target_compile_features(ohl_payload_path_tests PRIVATE cxx_std_20)
target_link_libraries(ohl_payload_path_tests PRIVATE OpenHalfLife::media)
ohl_enable_warnings(ohl_payload_path_tests)
add_test(NAME media.payload_path COMMAND ohl_payload_path_tests)

add_executable(ohl_payload_layout_tests
  media/payload_layout_test.cpp
)
target_compile_features(ohl_payload_layout_tests PRIVATE cxx_std_20)
target_link_libraries(ohl_payload_layout_tests PRIVATE OpenHalfLife::media)
ohl_enable_warnings(ohl_payload_layout_tests)
add_test(NAME media.payload_layout COMMAND ohl_payload_layout_tests)

add_executable(ohl_payload_stream_tests
  media/payload_stream_test.cpp
)
target_compile_features(ohl_payload_stream_tests PRIVATE cxx_std_20)
target_link_libraries(ohl_payload_stream_tests PRIVATE OpenHalfLife::media)
ohl_enable_warnings(ohl_payload_stream_tests)
add_test(NAME media.payload_stream COMMAND ohl_payload_stream_tests)

add_executable(ohl_payload_stage_tests
  media/payload_stage_test.cpp
)
target_compile_features(ohl_payload_stage_tests PRIVATE cxx_std_20)
target_link_libraries(ohl_payload_stage_tests PRIVATE OpenHalfLife::media)
ohl_enable_warnings(ohl_payload_stage_tests)
add_test(NAME media.payload_stage COMMAND ohl_payload_stage_tests)

if(OHL_ENABLE_EXPERIMENTAL_INSTALLSHIELD)
  add_executable(ohl_cabinet_tests
    media/installshield_cabinet_test.cpp
  )
  target_compile_features(ohl_cabinet_tests PRIVATE cxx_std_20)
  target_link_libraries(
    ohl_cabinet_tests
    PRIVATE
      OpenHalfLife::media
      OpenHalfLife::vfs
  )
  ohl_enable_warnings(ohl_cabinet_tests)
  add_test(NAME media.installshield_cabinet COMMAND ohl_cabinet_tests)
endif()

add_executable(ohl_vfs_tests
  vfs/udf_archive_test.cpp
)
target_compile_features(ohl_vfs_tests PRIVATE cxx_std_20)
target_link_libraries(ohl_vfs_tests PRIVATE OpenHalfLife::vfs)
ohl_enable_warnings(ohl_vfs_tests)
add_test(NAME vfs.udf_archive COMMAND ohl_vfs_tests)

add_test(
  NAME repository.policy
  COMMAND ${CMAKE_COMMAND} -P ${PROJECT_SOURCE_DIR}/cmake/CheckRepository.cmake
)

add_test(NAME app.help COMMAND open-half-life --help)
set_tests_properties(
  app.help
  PROPERTIES PASS_REGULAR_EXPRESSION "Usage:.*open-half-life"
)

add_test(NAME app.version COMMAND open-half-life --version)
set_tests_properties(
  app.version
  PROPERTIES PASS_REGULAR_EXPRESSION "Open Half-Life ${PROJECT_VERSION}"
)

add_test(
  NAME app.missing_iso
  COMMAND
    ${CMAKE_COMMAND}
    -DPROGRAM=$<TARGET_FILE:open-half-life>
    -DEXPECTED_RESULT=1
    "-DEXPECTED=Media preflight failed: file not found"
    -P ${CMAKE_CURRENT_SOURCE_DIR}/cmake/ExpectFailure.cmake
)

add_test(
  NAME app.missing_cache_option
  COMMAND
    ${CMAKE_COMMAND}
    -DPROGRAM=$<TARGET_FILE:open-half-life>
    "-DARGUMENTS=--cache"
    -DEXPECTED_RESULT=2
    "-DEXPECTED=--cache requires a path"
    -P ${CMAKE_CURRENT_SOURCE_DIR}/cmake/ExpectFailure.cmake
)
