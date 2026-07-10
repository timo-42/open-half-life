add_library(ohl_platform STATIC
  src/platform.cpp
  src/atomic_directory_store_unsupported.cpp
)

if(CMAKE_SYSTEM_NAME STREQUAL "Linux")
  target_sources(
    ohl_platform
    PRIVATE
      src/atomic_directory_store_linux.cpp
      src/atomic_directory_store_posix.cpp
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
