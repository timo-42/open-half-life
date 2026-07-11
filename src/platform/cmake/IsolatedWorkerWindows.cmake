include_guard(GLOBAL)

option(
  OHL_REQUIRE_WINDOWS_ISOLATED_WORKER
  "Fail configuration when the Windows x64 isolated-worker backend is unavailable"
  OFF
)

string(TOLOWER "${CMAKE_SYSTEM_PROCESSOR}" ohl_windows_worker_processor)
if(ohl_windows_worker_processor MATCHES "^(x86_64|amd64)$")
  target_sources(
    ohl_platform
    PRIVATE
      "${CMAKE_CURRENT_LIST_DIR}/../src/isolated_worker_windows.cpp"
      "${CMAKE_CURRENT_LIST_DIR}/../src/isolated_worker_windows_internal.hpp"
  )
  target_compile_definitions(
    ohl_platform
    PRIVATE
      NOMINMAX
      WIN32_LEAN_AND_MEAN
      _WIN32_WINNT=0x0A00
  )
  target_link_libraries(
    ohl_platform
    PRIVATE
      advapi32
      bcrypt
      userenv
  )
elseif(OHL_REQUIRE_WINDOWS_ISOLATED_WORKER)
  message(
    FATAL_ERROR
    "The required Windows isolated-worker backend supports x64 only; "
    "CMAKE_SYSTEM_PROCESSOR is '${CMAKE_SYSTEM_PROCESSOR}'"
  )
else()
  target_sources(ohl_platform PRIVATE src/isolated_worker_unsupported.cpp)
endif()
