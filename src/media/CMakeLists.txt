add_library(ohl_media STATIC
  src/import_cache.cpp
  src/iso_inspector.cpp
)

add_library(OpenHalfLife::media ALIAS ohl_media)

target_compile_features(ohl_media PUBLIC cxx_std_20)
target_include_directories(
  ohl_media
  PUBLIC
    $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/include>
)
target_link_libraries(ohl_media PRIVATE OpenHalfLife::core)
if(OHL_ENABLE_EXPERIMENTAL_INSTALLSHIELD)
  target_sources(ohl_media PRIVATE src/installshield_cabinet.cpp)
  target_link_libraries(
    ohl_media
    PRIVATE
      OpenHalfLife::vfs
      ThirdParty::unshield
  )
endif()
ohl_enable_warnings(ohl_media)
