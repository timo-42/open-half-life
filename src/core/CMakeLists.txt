add_library(ohl_core STATIC
  src/log.cpp
)

add_library(OpenHalfLife::core ALIAS ohl_core)

target_compile_features(ohl_core PUBLIC cxx_std_20)
target_include_directories(
  ohl_core
  PUBLIC
    $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/include>
)
target_link_libraries(ohl_core PUBLIC Threads::Threads)
ohl_enable_warnings(ohl_core)
