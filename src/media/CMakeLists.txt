add_library(ohl_media STATIC
  src/cancellation.cpp
  src/import_cache.cpp
  src/iso_inspector.cpp
  src/payload_layout.cpp
  src/payload_path.cpp
  src/payload_stage.cpp
  src/payload_stream.cpp
  src/source_stability.cpp
)

add_library(OpenHalfLife::media ALIAS ohl_media)

target_compile_features(ohl_media PUBLIC cxx_std_20)
target_include_directories(
  ohl_media
  PUBLIC
    $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/include>
)
target_link_libraries(
  ohl_media
  PUBLIC OpenHalfLife::platform
  PRIVATE OpenHalfLife::core
)
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

add_library(ohl_media_parser_results STATIC
  src/parser_result_session.cpp
)

add_library(
  OpenHalfLife::media_parser_results ALIAS ohl_media_parser_results
)

target_compile_features(ohl_media_parser_results PUBLIC cxx_std_20)
target_include_directories(
  ohl_media_parser_results
  PUBLIC
    $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/include>
)
target_link_libraries(
  ohl_media_parser_results
  PUBLIC
    OpenHalfLife::media
    OpenHalfLife::parser
)
ohl_enable_warnings(ohl_media_parser_results)

add_library(ohl_media_parser_reads STATIC
  src/parser_source_read_broker.cpp
)

add_library(OpenHalfLife::media_parser_reads ALIAS ohl_media_parser_reads)

target_compile_features(ohl_media_parser_reads PUBLIC cxx_std_20)
target_include_directories(
  ohl_media_parser_reads
  PUBLIC
    $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/include>
)
target_link_libraries(
  ohl_media_parser_reads
  PUBLIC OpenHalfLife::media_parser_results
)
ohl_enable_warnings(ohl_media_parser_reads)

add_library(ohl_media_parser_transport STATIC
  src/parser_frame_channel.cpp
)

add_library(
  OpenHalfLife::media_parser_transport ALIAS ohl_media_parser_transport
)

target_compile_features(ohl_media_parser_transport PUBLIC cxx_std_20)
target_include_directories(
  ohl_media_parser_transport
  PUBLIC
    $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/include>
)
target_link_libraries(
  ohl_media_parser_transport
  PUBLIC
    OpenHalfLife::parser
    OpenHalfLife::platform
    Threads::Threads
)
ohl_enable_warnings(ohl_media_parser_transport)
