add_executable(ohl_core_tests
  core/log_test.cpp
)
target_compile_features(ohl_core_tests PRIVATE cxx_std_20)
target_link_libraries(ohl_core_tests PRIVATE OpenHalfLife::core)
ohl_enable_warnings(ohl_core_tests)
add_test(NAME core.logging COMMAND ohl_core_tests)

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

add_executable(ohl_media_tests
  media/iso_inspector_test.cpp
)
target_compile_features(ohl_media_tests PRIVATE cxx_std_20)
target_link_libraries(ohl_media_tests PRIVATE OpenHalfLife::media)
ohl_enable_warnings(ohl_media_tests)
add_test(NAME media.iso_inspector COMMAND ohl_media_tests)

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
    "-DEXPECTED=Media preflight failed: file not found"
    -P ${CMAKE_CURRENT_SOURCE_DIR}/cmake/ExpectFailure.cmake
)
