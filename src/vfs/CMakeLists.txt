add_library(ohl_vfs STATIC
  src/udf_archive.cpp
)

add_library(OpenHalfLife::vfs ALIAS ohl_vfs)

target_compile_features(ohl_vfs PUBLIC cxx_std_20)
target_include_directories(
  ohl_vfs
  PUBLIC
    $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/include>
)
target_link_libraries(ohl_vfs PRIVATE ThirdParty::udfread)
ohl_enable_warnings(ohl_vfs)
