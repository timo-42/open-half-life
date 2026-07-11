add_library(ohl_platform STATIC
  src/platform.cpp
  src/atomic_directory_store_unsupported.cpp
  src/isolated_worker.cpp
)

string(TOLOWER "${CMAKE_SYSTEM_PROCESSOR}" ohl_platform_system_processor)
if(CMAKE_SYSTEM_NAME STREQUAL "Linux" AND
   ohl_platform_system_processor MATCHES "^(x86_64|amd64)$")
  include(cmake/IsolatedWorkerLinux.cmake)
else()
  target_sources(ohl_platform PRIVATE src/isolated_worker_unsupported.cpp)
endif()

if(CMAKE_SYSTEM_NAME STREQUAL "Linux")
  target_sources(
    ohl_platform
    PRIVATE
      src/atomic_directory_store_linux.cpp
      src/atomic_directory_store_posix.cpp
      src/media_source_posix.cpp
  )
elseif(CMAKE_SYSTEM_NAME STREQUAL "Darwin")
  target_sources(ohl_platform PRIVATE src/media_source_posix.cpp)
elseif(CMAKE_SYSTEM_NAME STREQUAL "Windows")
  target_sources(ohl_platform PRIVATE src/media_source_windows.cpp)
else()
  target_sources(ohl_platform PRIVATE src/media_source_unsupported.cpp)
  target_compile_definitions(
    ohl_platform
    PUBLIC OHL_PLATFORM_MEDIA_SOURCE_UNSUPPORTED=1
  )
endif()

add_library(OpenHalfLife::platform ALIAS ohl_platform)

target_compile_features(ohl_platform PUBLIC cxx_std_20)
target_include_directories(
  ohl_platform
  PUBLIC
    $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/include>
)
ohl_enable_warnings(ohl_platform)
