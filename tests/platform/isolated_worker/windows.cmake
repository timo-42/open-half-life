if(NOT CMAKE_SYSTEM_NAME STREQUAL "Windows")
  return()
endif()

string(TOLOWER "${CMAKE_SYSTEM_PROCESSOR}" ohl_windows_worker_test_processor)
if(NOT ohl_windows_worker_test_processor MATCHES "^(x86_64|amd64)$")
  return()
endif()

function(ohl_add_windows_isolated_worker_helper suffix mode)
  set(target "ohl_isolated_worker_windows_worker_${suffix}")
  add_executable(
    ${target}
    "${CMAKE_CURRENT_LIST_DIR}/../isolated_worker_windows_worker.cpp"
  )
  target_compile_features(${target} PRIVATE cxx_std_20)
  target_compile_definitions(
    ${target}
    PRIVATE
      NOMINMAX
      WIN32_LEAN_AND_MEAN
      _WIN32_WINNT=0x0A00
      OHL_WINDOWS_TEST_WORKER_MODE=${mode}
  )
  target_include_directories(
    ${target} PRIVATE "${PROJECT_SOURCE_DIR}/src/platform/src"
  )
  set_target_properties(
    ${target}
    PROPERTIES
      OUTPUT_NAME "ohl_media_parser_worker"
      RUNTIME_OUTPUT_DIRECTORY
        "${CMAKE_CURRENT_BINARY_DIR}/isolated-worker-windows-${suffix}"
      MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>"
  )
  target_link_libraries(${target} PRIVATE advapi32)
  ohl_enable_warnings(${target})
endfunction()

ohl_add_windows_isolated_worker_helper(ready 0)
ohl_add_windows_isolated_worker_helper(bad_ready 1)
ohl_add_windows_isolated_worker_helper(no_ready 2)
ohl_add_windows_isolated_worker_helper(exit 3)

add_executable(
  ohl_isolated_worker_windows_tests
  "${CMAKE_CURRENT_LIST_DIR}/../isolated_worker_windows_test.cpp"
)
target_compile_features(ohl_isolated_worker_windows_tests PRIVATE cxx_std_20)
target_compile_definitions(
  ohl_isolated_worker_windows_tests
  PRIVATE
    NOMINMAX
    WIN32_LEAN_AND_MEAN
    _WIN32_WINNT=0x0A00
    OHL_WINDOWS_TEST_WORKER_READY_PATH=L"$<TARGET_FILE:ohl_isolated_worker_windows_worker_ready>"
    OHL_WINDOWS_TEST_WORKER_BAD_READY_PATH=L"$<TARGET_FILE:ohl_isolated_worker_windows_worker_bad_ready>"
    OHL_WINDOWS_TEST_WORKER_NO_READY_PATH=L"$<TARGET_FILE:ohl_isolated_worker_windows_worker_no_ready>"
    OHL_WINDOWS_TEST_WORKER_EXIT_PATH=L"$<TARGET_FILE:ohl_isolated_worker_windows_worker_exit>"
)
target_include_directories(
  ohl_isolated_worker_windows_tests
  PRIVATE
    "${PROJECT_SOURCE_DIR}/src/platform/include"
    "${PROJECT_SOURCE_DIR}/src/platform/src"
)
target_link_libraries(
  ohl_isolated_worker_windows_tests PRIVATE OpenHalfLife::platform
)
add_dependencies(
  ohl_isolated_worker_windows_tests
  ohl_isolated_worker_windows_worker_ready
  ohl_isolated_worker_windows_worker_bad_ready
  ohl_isolated_worker_windows_worker_no_ready
  ohl_isolated_worker_windows_worker_exit
)
ohl_enable_warnings(ohl_isolated_worker_windows_tests)
add_test(
  NAME platform.isolated_worker.windows
  COMMAND ohl_isolated_worker_windows_tests
)
set_tests_properties(
  platform.isolated_worker.windows
  PROPERTIES
    TIMEOUT 30
    LABELS windows-isolated-worker
    RUN_SERIAL TRUE
)
