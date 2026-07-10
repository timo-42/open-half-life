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

add_executable(ohl_import_cache_tests
  media/import_cache_test.cpp
)
target_compile_features(ohl_import_cache_tests PRIVATE cxx_std_20)
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
