cmake_minimum_required(VERSION 3.25)

project(ohl_render_dependency_probe LANGUAGES C CXX)

if(NOT DEFINED OHL_RENDER_DEPS_SOURCE_DIR OR
   NOT IS_ABSOLUTE "${OHL_RENDER_DEPS_SOURCE_DIR}")
    message(FATAL_ERROR
        "OHL_RENDER_DEPS_SOURCE_DIR must be an absolute path to the repository"
    )
endif()
if(NOT EXISTS "${OHL_RENDER_DEPS_SOURCE_DIR}/cmake/RenderDependencies.cmake")
    message(FATAL_ERROR
        "OHL_RENDER_DEPS_SOURCE_DIR does not contain cmake/RenderDependencies.cmake"
    )
endif()

set(OHL_RENDER_DEPS_USE_SYNTHETIC_FIXTURES ON)
set(OHL_RENDER_DEPS_FETCH OFF)
set(OHL_RENDER_DEPS_SDL_MODE SYSTEM)
set(OHL_RENDER_DEPS_FIXTURE_SET valid)

if(NOT OHL_RENDER_DEPS_NEGATIVE_CASE)
    include("${CMAKE_CURRENT_SOURCE_DIR}/fixtures/prepare_runtime.cmake")
    ohl_prepare_render_dependency_fixture_runtime(
        "${CMAKE_CURRENT_BINARY_DIR}/synthetic-runtime"
    )
endif()

if(OHL_RENDER_DEPS_NEGATIVE_CASE STREQUAL "override")
    set(FETCHCONTENT_SOURCE_DIR_OHL_VOLK "/provenance/substitution")
elseif(OHL_RENDER_DEPS_NEGATIVE_CASE STREQUAL "preexisting")
    add_library(ohl_third_party_sdl3 INTERFACE)
elseif(OHL_RENDER_DEPS_NEGATIVE_CASE STREQUAL "upstream")
    add_library(SDL3::SDL3-shared SHARED IMPORTED)
elseif(OHL_RENDER_DEPS_NEGATIVE_CASE STREQUAL "static")
    set(OHL_RENDER_DEPS_FIXTURE_SET static)
elseif(OHL_RENDER_DEPS_NEGATIVE_CASE STREQUAL "tampered")
    set(OHL_RENDER_DEPS_FIXTURE_SET tampered)
elseif(OHL_RENDER_DEPS_NEGATIVE_CASE STREQUAL "shadowed")
    set(OHL_RENDER_DEPS_FIXTURE_SET shadowed)
elseif(OHL_RENDER_DEPS_NEGATIVE_CASE STREQUAL "forbidden")
    set(OHL_RENDER_DEPS_FIXTURE_SET forbidden)
elseif(OHL_RENDER_DEPS_NEGATIVE_CASE STREQUAL "archive")
    set(OHL_RENDER_DEPS_FIXTURE_SET archive)
elseif(OHL_RENDER_DEPS_NEGATIVE_CASE STREQUAL "missing")
    set(OHL_RENDER_DEPS_FIXTURE_SET missing)
elseif(OHL_RENDER_DEPS_NEGATIVE_CASE STREQUAL "bad-fetch")
    set(OHL_RENDER_DEPS_FETCH MAYBE)
elseif(OHL_RENDER_DEPS_NEGATIVE_CASE STREQUAL "official-substitution")
    set(OHL_RENDER_DEPS_SDL_MODE OFFICIAL_BINARY)
endif()

include("${OHL_RENDER_DEPS_SOURCE_DIR}/cmake/RenderDependencies.cmake")
ohl_add_render_dependencies()

function(assert_target_property target property expected)
    get_target_property(_actual "${target}" "${property}")
    if(NOT "${_actual}" STREQUAL "${expected}")
        message(FATAL_ERROR
            "${target}.${property}: expected '${expected}', got '${_actual}'"
        )
    endif()
endfunction()

foreach(_target IN ITEMS
    ThirdParty::SDL3
    ThirdParty::VulkanHeaders
    ThirdParty::volk
    ohl_third_party_sdl3
    ohl_third_party_vulkan_headers
    ohl_third_party_volk
    ohl_volk_impl
)
    if(NOT TARGET "${_target}")
        message(FATAL_ERROR "Expected render dependency target ${_target}")
    endif()
endforeach()

assert_target_property(
    ohl_third_party_sdl3 OHL_PROVENANCE_KIND "synthetic-test-fixture"
)
assert_target_property(ohl_third_party_sdl3 OHL_PACKAGE_VERSION "3.4.10")
assert_target_property(ohl_third_party_sdl3 OHL_LINKAGE "shared")
assert_target_property(
    ohl_third_party_sdl3 OHL_VULKAN_LOADER_SOURCE
    "SDL_Vulkan_GetVkGetInstanceProcAddr"
)
foreach(_property IN ITEMS OHL_PINNED_VERSION OHL_PINNED_SHA256 OHL_SOURCE_URL)
    get_target_property(_unexpected ohl_third_party_sdl3 "${_property}")
    if(NOT _unexpected STREQUAL "_unexpected-NOTFOUND")
        message(FATAL_ERROR
            "Unpinned SDL system mode must not publish ${_property}: ${_unexpected}"
        )
    endif()
endforeach()

assert_target_property(
    ohl_third_party_vulkan_headers OHL_PROVENANCE_KIND "synthetic-test-fixture"
)
assert_target_property(
    ohl_third_party_vulkan_headers OHL_ACQUISITION_KIND
    "individual-file-allowlist"
)
assert_target_property(ohl_third_party_vulkan_headers OHL_PRODUCTION_PIN_COMMIT
    "8864cdc896bbc2a9b6eb36b3218fc9ef57908d77"
)
assert_target_property(ohl_third_party_vulkan_headers OHL_VULKAN_API_BASELINE "1.1")
if(NOT OHL_VULKAN_HEADERS_LICENSE_FILES STREQUAL
   "${OHL_RENDER_DEPS_SOURCE_DIR}/tests/cmake/render_dependencies/fixtures/valid/vulkan/LICENSE.md;${OHL_RENDER_DEPS_SOURCE_DIR}/tests/cmake/render_dependencies/fixtures/valid/vulkan/LICENSES/Apache-2.0.txt")
    message(FATAL_ERROR "Vulkan license payload list is incomplete")
endif()
foreach(_target IN ITEMS ohl_third_party_vulkan_headers ohl_third_party_volk)
    foreach(_property IN ITEMS OHL_PINNED_VERSION OHL_PINNED_COMMIT OHL_SOURCE_URL)
        get_target_property(_unexpected "${_target}" "${_property}")
        if(NOT _unexpected STREQUAL "_unexpected-NOTFOUND")
            message(FATAL_ERROR
                "Synthetic fixture target must not publish ${_property}: ${_unexpected}"
            )
        endif()
    endforeach()
endforeach()

assert_target_property(
    ohl_third_party_volk OHL_PROVENANCE_KIND "synthetic-test-fixture"
)
assert_target_property(ohl_third_party_volk OHL_PRODUCTION_PIN_COMMIT
    "3ca312a4f38baa63d8006b6905abbeeb89c8087d"
)
assert_target_property(
    ohl_third_party_volk OHL_INITIALIZATION_ENTRYPOINT "volkInitializeCustom"
)
assert_target_property(ohl_third_party_volk OHL_DIRECT_VULKAN_LOADER_LINK "FALSE")

if(TARGET Vulkan::Vulkan OR TARGET ThirdParty::VulkanLoader)
    message(FATAL_ERROR "The dependency wrapper must not publish a Vulkan loader")
endif()
if(TARGET SDL3::SDL3-static)
    message(FATAL_ERROR "The positive fixture unexpectedly published static SDL")
endif()
if(APPLE)
    if(NOT TARGET ThirdParty::MoltenVKRuntime OR
       NOT TARGET ohl_third_party_moltenvk_runtime)
        message(FATAL_ERROR "Apple probe requires external dynamic MoltenVK")
    endif()
    assert_target_property(
        ohl_third_party_moltenvk_runtime OHL_PROVENANCE_KIND
        "unpinned-external-dynamic"
    )
    assert_target_property(
        ohl_third_party_moltenvk_runtime OHL_RUNTIME_KIND
        "external-dynamic-dylib"
    )
    foreach(_property IN ITEMS OHL_PINNED_VERSION OHL_PINNED_SHA256 OHL_SOURCE_URL)
        get_target_property(_unexpected ohl_third_party_moltenvk_runtime "${_property}")
        if(NOT _unexpected STREQUAL "_unexpected-NOTFOUND")
            message(FATAL_ERROR "External MoltenVK must not publish ${_property}")
        endif()
    endforeach()
else()
    if(TARGET ThirdParty::MoltenVKRuntime OR TARGET ohl_third_party_moltenvk_runtime)
        message(FATAL_ERROR "MoltenVK must only be published on Apple platforms")
    endif()
endif()

add_executable(render_dependency_probe main.cpp)
target_compile_features(render_dependency_probe PRIVATE cxx_std_20)
target_link_libraries(render_dependency_probe PRIVATE
    ThirdParty::SDL3
    ThirdParty::VulkanHeaders
    ThirdParty::volk
)
if(APPLE)
    target_link_libraries(render_dependency_probe PRIVATE ThirdParty::MoltenVKRuntime)
endif()

add_executable(macho_fixture_generator macho_fixture_generator.cpp)
target_compile_features(macho_fixture_generator PRIVATE cxx_std_20)
set(_macho_fixture_dir "${CMAKE_CURRENT_BINARY_DIR}/macho-fixtures")
add_custom_command(
    OUTPUT "${_macho_fixture_dir}/stamp"
    COMMAND "${CMAKE_COMMAND}" -E rm -rf "${_macho_fixture_dir}"
    COMMAND "$<TARGET_FILE:macho_fixture_generator>" "${_macho_fixture_dir}"
    COMMAND "${CMAKE_COMMAND}" -E touch "${_macho_fixture_dir}/stamp"
    DEPENDS macho_fixture_generator
    VERBATIM
)
add_custom_target(macho_fixtures DEPENDS "${_macho_fixture_dir}/stamp")
add_dependencies(render_dependency_probe macho_fixtures)

enable_testing()
add_test(
    NAME render_dependencies_positive_offline
    COMMAND render_dependency_probe
)
foreach(_macho_case IN ITEMS
    thin-arm64.dylib
    universal-arm64-x86_64.dylib
    universal-little.dylib
    universal-fat64.dylib
    universal-fat64-little.dylib
)
    string(REPLACE ".dylib" "" _macho_name "${_macho_case}")
    add_test(
        NAME "render_dependencies_macho_accept_${_macho_name}"
        COMMAND "${CMAKE_COMMAND}"
            "-DOHL_RENDER_DEPS_SOURCE_DIR=${OHL_RENDER_DEPS_SOURCE_DIR}"
            "-DMACHO_FIXTURE_DIR=${_macho_fixture_dir}"
            "-DMACHO_CASE=${_macho_case}"
            -P "${CMAKE_CURRENT_SOURCE_DIR}/macho_case.cmake"
    )
endforeach()
find_program(_ninja_executable NAMES ninja REQUIRED)
if(WIN32)
    set(_expected_sdl_link "${OHL_RENDER_DEPS_FIXTURE_SDL_IMPLIB}")
else()
    set(_expected_sdl_link "${OHL_RENDER_DEPS_FIXTURE_SDL_RUNTIME}")
endif()
add_test(
    NAME render_dependencies_final_link_surface
    COMMAND "${CMAKE_COMMAND}"
        "-DNINJA_EXECUTABLE=${_ninja_executable}"
        "-DBUILD_DIR=${CMAKE_CURRENT_BINARY_DIR}"
        "-DEXPECTED_SDL_LINK=${_expected_sdl_link}"
        "-DEXPECTED_PLATFORM_LIBS=${CMAKE_DL_LIBS}"
        "-DEXPECTED_MOLTENVK=${OHL_RENDER_DEPS_MOLTENVK_LIBRARY}"
        -P "${CMAKE_CURRENT_SOURCE_DIR}/verify_link_surface.cmake"
)
add_test(
    NAME render_dependencies_windows_processor_matrix
    COMMAND "${CMAKE_COMMAND}"
        "-DOHL_RENDER_DEPS_SOURCE_DIR=${OHL_RENDER_DEPS_SOURCE_DIR}"
        -P "${CMAKE_CURRENT_SOURCE_DIR}/processor_matrix.cmake"
)

function(add_negative_probe name case expected)
    add_test(
        NAME "render_dependencies_reject_${name}"
        COMMAND "${CMAKE_COMMAND}"
            "-DOHL_RENDER_DEPS_SOURCE_DIR=${OHL_RENDER_DEPS_SOURCE_DIR}"
            "-DPROBE_SOURCE_DIR=${CMAKE_CURRENT_SOURCE_DIR}"
            "-DPROBE_BINARY_DIR=${CMAKE_CURRENT_BINARY_DIR}/negative/${name}"
            "-DNEGATIVE_CASE=${case}"
            "-DEXPECTED=${expected}"
            "-DMACHO_FIXTURE_DIR=${_macho_fixture_dir}"
            -P "${CMAKE_CURRENT_SOURCE_DIR}/run_negative.cmake"
    )
endfunction()

add_negative_probe(provider provider "Dependency providers")
add_negative_probe(fetchcontent_override override "source override is forbidden")
add_negative_probe(preexisting_wrapper preexisting "Pre-existing render dependency target")
add_negative_probe(preexisting_upstream upstream "Pre-existing upstream dependency target")
add_negative_probe(static_sdl static "must be a shared library target")
add_negative_probe(tampered_fixture tampered "dependency file hash mismatch")
add_negative_probe(shadowed_vulkan_header shadowed "outside the audited volk allowlist")
add_negative_probe(forbidden_vulkan forbidden "Forbidden Vulkan loader header")
add_negative_probe(vulkan_archive archive "Vulkan-Headers archive is forbidden")
add_negative_probe(offline_missing offline-real "audited dependency file is missing")
add_negative_probe(invalid_fetch bad-fetch "must be exactly ON or OFF")
add_negative_probe(
    sdl_archive_member_extra archive-member-extra "archive member manifest mismatch"
)
add_negative_probe(
    sdl_static_surface static-surface "Static or unrelated SDL archive member"
)
add_negative_probe(macho_thin_x86 macho-thin-x86 "not arm64")
add_negative_probe(macho_no_arm64 macho-no-arm64 "has no arm64")
add_negative_probe(macho_arm_wrong_type macho-arm-wrong-type "not MH_DYLIB")
add_negative_probe(macho_fat_wrong_type macho-fat-wrong-type "not MH_DYLIB")
add_negative_probe(macho_malformed_count macho-malformed-count "architecture count")
add_negative_probe(macho_truncated_table macho-truncated-table "Truncated")
add_negative_probe(macho_truncated_slice macho-truncated-slice "Truncated")
add_negative_probe(macho_truncated_thin macho-truncated-thin "Truncated")
add_negative_probe(macho_overflow macho-overflow "Overflowing")
add_negative_probe(macho_overflowing_slice macho-overflowing-slice "Overflowing")
add_negative_probe(macho_slice_inside_table macho-slice-inside-table "slice bounds")
add_negative_probe(macho_static_archive macho-static "not a supported Mach-O")
add_negative_probe(
    official_provenance_substitution official-substitution
    "cannot substitute official SDL binary provenance"
)
