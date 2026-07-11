option(
  OHL_REQUIRE_LINUX_ISOLATED_WORKER
  "Fail configuration when the Linux x64 isolated-worker backend is unavailable"
  OFF
)

string(TOLOWER "${CMAKE_SYSTEM_PROCESSOR}" ohl_linux_worker_processor)
if(ohl_linux_worker_processor MATCHES "^(x86_64|amd64)$")
  target_sources(ohl_platform PRIVATE src/isolated_worker_linux.cpp)
elseif(OHL_REQUIRE_LINUX_ISOLATED_WORKER)
  message(
    FATAL_ERROR
    "The required Linux isolated-worker backend supports x86-64 only; "
    "CMAKE_SYSTEM_PROCESSOR is '${CMAKE_SYSTEM_PROCESSOR}'"
  )
else()
  target_sources(ohl_platform PRIVATE src/isolated_worker_unsupported.cpp)
endif()
