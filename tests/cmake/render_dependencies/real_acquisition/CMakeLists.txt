cmake_minimum_required(VERSION 3.25)

project(ohl_render_dependency_real_acquisition_probe LANGUAGES C CXX)

if(NOT DEFINED OHL_RENDER_DEPS_SOURCE_DIR OR
   NOT IS_ABSOLUTE "${OHL_RENDER_DEPS_SOURCE_DIR}")
    message(FATAL_ERROR "OHL_RENDER_DEPS_SOURCE_DIR must be absolute")
endif()

# SDL remains an unpinned system-style package fixture here. This auxiliary
# probe exercises only the production pinned Vulkan/volk acquisition path.
list(PREPEND CMAKE_PREFIX_PATH
    "${OHL_RENDER_DEPS_SOURCE_DIR}/tests/cmake/render_dependencies/fixtures/valid/sdl"
)
include(
    "${OHL_RENDER_DEPS_SOURCE_DIR}/tests/cmake/render_dependencies/fixtures/prepare_runtime.cmake"
)
ohl_prepare_render_dependency_fixture_runtime(
    "${CMAKE_CURRENT_BINARY_DIR}/synthetic-runtime"
)
set(OHL_RENDER_DEPS_USE_SYNTHETIC_FIXTURES OFF)
set(OHL_RENDER_DEPS_SDL_MODE SYSTEM)

include("${OHL_RENDER_DEPS_SOURCE_DIR}/cmake/RenderDependencies.cmake")
ohl_add_render_dependencies()

get_target_property(
    _vulkan_provenance ohl_third_party_vulkan_headers OHL_PROVENANCE_KIND
)
if(NOT _vulkan_provenance STREQUAL "official-pinned-file-allowlist")
    message(FATAL_ERROR "Production Vulkan provenance was not published")
endif()
get_target_property(_volk_commit ohl_third_party_volk OHL_PINNED_COMMIT)
if(NOT _volk_commit STREQUAL "3ca312a4f38baa63d8006b6905abbeeb89c8087d")
    message(FATAL_ERROR "Production volk pin was not published")
endif()

add_executable(render_dependency_real_acquisition_probe ../main.cpp)
target_link_libraries(render_dependency_real_acquisition_probe PRIVATE
    ThirdParty::SDL3
    ThirdParty::VulkanHeaders
    ThirdParty::volk
)
if(APPLE)
    target_link_libraries(
        render_dependency_real_acquisition_probe PRIVATE ThirdParty::MoltenVKRuntime
    )
endif()
