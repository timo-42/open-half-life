function(ohl_add_linux_isolated_worker_helper suffix mode)
  set(target "ohl_isolated_worker_linux_helper_${suffix}")
  add_executable(
    ${target}
    ${CMAKE_CURRENT_LIST_DIR}/../isolated_worker_linux_worker.cpp
  )
  target_compile_features(${target} PRIVATE cxx_std_20)
  target_include_directories(
    ${target} PRIVATE ${PROJECT_SOURCE_DIR}/src/platform/src
  )
  target_compile_definitions(
    ${target}
    PRIVATE
      OHL_LINUX_ISOLATED_WORKER_FREESTANDING=1
      OHL_LINUX_TEST_WORKER_MODE=${mode}
  )
  target_compile_options(
    ${target}
    PRIVATE
      -ffreestanding
      -fno-exceptions
      -fno-rtti
      -fno-stack-protector
      -fno-pie
  )
  target_link_options(
    ${target} PRIVATE -nostdlib -static -no-pie -Wl,-e,_start
  )
  ohl_enable_warnings(${target})
endfunction()

ohl_add_linux_isolated_worker_helper(ready 0)
ohl_add_linux_isolated_worker_helper(bad_ready 1)
ohl_add_linux_isolated_worker_helper(no_ready 2)
ohl_add_linux_isolated_worker_helper(forbidden_stat 3)
ohl_add_linux_isolated_worker_helper(forbidden_open 4)
ohl_add_linux_isolated_worker_helper(forbidden_network 5)
ohl_add_linux_isolated_worker_helper(forbidden_fork 6)
ohl_add_linux_isolated_worker_helper(forbidden_reexec 7)
ohl_add_linux_isolated_worker_helper(ready_timeout 8)
ohl_add_linux_isolated_worker_helper(ready_held_open 9)
ohl_add_linux_isolated_worker_helper(ready_trailing 10)
ohl_add_linux_isolated_worker_helper(ready_truncated 11)
ohl_add_linux_isolated_worker_helper(execveat_high_dirfd 12)
ohl_add_linux_isolated_worker_helper(execveat_wrong_flags 13)
ohl_add_linux_isolated_worker_helper(execveat_high_flags 14)

set(
  ohl_linux_test_worker_path
  "${CMAKE_CURRENT_BINARY_DIR}/isolated-worker-test-root/ohl-media-parser-worker"
)

add_library(
  ohl_isolated_worker_linux_test_backend STATIC
  ${PROJECT_SOURCE_DIR}/src/platform/src/isolated_worker.cpp
  ${PROJECT_SOURCE_DIR}/src/platform/src/isolated_worker_linux.cpp
)
target_compile_features(
  ohl_isolated_worker_linux_test_backend PUBLIC cxx_std_20
)
target_include_directories(
  ohl_isolated_worker_linux_test_backend
  PUBLIC ${PROJECT_SOURCE_DIR}/src/platform/include
  PRIVATE ${PROJECT_SOURCE_DIR}/src/platform/src
)
target_compile_definitions(
  ohl_isolated_worker_linux_test_backend
  PRIVATE
    OHL_LINUX_ISOLATED_WORKER_TESTING=1
    OHL_LINUX_TEST_MEDIA_PARSER_WORKER_PATH="${ohl_linux_test_worker_path}"
)
target_link_libraries(
  ohl_isolated_worker_linux_test_backend PUBLIC Threads::Threads
)
ohl_enable_warnings(ohl_isolated_worker_linux_test_backend)

add_executable(
  ohl_isolated_worker_linux_tests
  ${CMAKE_CURRENT_LIST_DIR}/../isolated_worker_linux_test.cpp
)
target_compile_features(ohl_isolated_worker_linux_tests PRIVATE cxx_std_20)
target_include_directories(
  ohl_isolated_worker_linux_tests
  PRIVATE ${PROJECT_SOURCE_DIR}/src/platform/src
)
target_compile_definitions(
  ohl_isolated_worker_linux_tests
  PRIVATE
    OHL_LINUX_TEST_WORKER_STAGE_PATH="${ohl_linux_test_worker_path}"
    OHL_LINUX_TEST_WORKER_READY_PATH="$<TARGET_FILE:ohl_isolated_worker_linux_helper_ready>"
    OHL_LINUX_TEST_WORKER_BAD_READY_PATH="$<TARGET_FILE:ohl_isolated_worker_linux_helper_bad_ready>"
    OHL_LINUX_TEST_WORKER_NO_READY_PATH="$<TARGET_FILE:ohl_isolated_worker_linux_helper_no_ready>"
    OHL_LINUX_TEST_WORKER_FORBIDDEN_STAT_PATH="$<TARGET_FILE:ohl_isolated_worker_linux_helper_forbidden_stat>"
    OHL_LINUX_TEST_WORKER_FORBIDDEN_OPEN_PATH="$<TARGET_FILE:ohl_isolated_worker_linux_helper_forbidden_open>"
    OHL_LINUX_TEST_WORKER_FORBIDDEN_NETWORK_PATH="$<TARGET_FILE:ohl_isolated_worker_linux_helper_forbidden_network>"
    OHL_LINUX_TEST_WORKER_FORBIDDEN_FORK_PATH="$<TARGET_FILE:ohl_isolated_worker_linux_helper_forbidden_fork>"
    OHL_LINUX_TEST_WORKER_FORBIDDEN_REEXEC_PATH="$<TARGET_FILE:ohl_isolated_worker_linux_helper_forbidden_reexec>"
    OHL_LINUX_TEST_WORKER_READY_TIMEOUT_PATH="$<TARGET_FILE:ohl_isolated_worker_linux_helper_ready_timeout>"
    OHL_LINUX_TEST_WORKER_READY_HELD_OPEN_PATH="$<TARGET_FILE:ohl_isolated_worker_linux_helper_ready_held_open>"
    OHL_LINUX_TEST_WORKER_READY_TRAILING_PATH="$<TARGET_FILE:ohl_isolated_worker_linux_helper_ready_trailing>"
    OHL_LINUX_TEST_WORKER_READY_TRUNCATED_PATH="$<TARGET_FILE:ohl_isolated_worker_linux_helper_ready_truncated>"
    OHL_LINUX_TEST_WORKER_EXECVEAT_HIGH_DIRFD_PATH="$<TARGET_FILE:ohl_isolated_worker_linux_helper_execveat_high_dirfd>"
    OHL_LINUX_TEST_WORKER_EXECVEAT_WRONG_FLAGS_PATH="$<TARGET_FILE:ohl_isolated_worker_linux_helper_execveat_wrong_flags>"
    OHL_LINUX_TEST_WORKER_EXECVEAT_HIGH_FLAGS_PATH="$<TARGET_FILE:ohl_isolated_worker_linux_helper_execveat_high_flags>"
)
target_link_libraries(
  ohl_isolated_worker_linux_tests
  PRIVATE ohl_isolated_worker_linux_test_backend
)
add_dependencies(
  ohl_isolated_worker_linux_tests
  ohl_isolated_worker_linux_helper_ready
  ohl_isolated_worker_linux_helper_bad_ready
  ohl_isolated_worker_linux_helper_no_ready
  ohl_isolated_worker_linux_helper_forbidden_stat
  ohl_isolated_worker_linux_helper_forbidden_open
  ohl_isolated_worker_linux_helper_forbidden_network
  ohl_isolated_worker_linux_helper_forbidden_fork
  ohl_isolated_worker_linux_helper_forbidden_reexec
  ohl_isolated_worker_linux_helper_ready_timeout
  ohl_isolated_worker_linux_helper_ready_held_open
  ohl_isolated_worker_linux_helper_ready_trailing
  ohl_isolated_worker_linux_helper_ready_truncated
  ohl_isolated_worker_linux_helper_execveat_high_dirfd
  ohl_isolated_worker_linux_helper_execveat_wrong_flags
  ohl_isolated_worker_linux_helper_execveat_high_flags
)
ohl_enable_warnings(ohl_isolated_worker_linux_tests)
add_test(
  NAME platform.isolated_worker.linux
  COMMAND ohl_isolated_worker_linux_tests
)
set_tests_properties(
  platform.isolated_worker.linux
  PROPERTIES TIMEOUT 30 LABELS linux-isolated-worker
)
