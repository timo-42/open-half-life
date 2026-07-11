include("${OHL_RENDER_DEPS_SOURCE_DIR}/cmake/RenderDependencies.cmake")

if(NEGATIVE_CASE STREQUAL "archive-member-extra")
    file(REMOVE_RECURSE "${NEGATIVE_BINARY_DIR}")
    file(MAKE_DIRECTORY "${NEGATIVE_BINARY_DIR}/input")
    file(WRITE "${NEGATIVE_BINARY_DIR}/input/allowed.txt" "allowed\n")
    file(WRITE "${NEGATIVE_BINARY_DIR}/input/extra.txt" "extra\n")
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -E tar cf "${NEGATIVE_BINARY_DIR}/fixture.zip"
            --format=zip allowed.txt extra.txt
        WORKING_DIRECTORY "${NEGATIVE_BINARY_DIR}/input"
        COMMAND_ERROR_IS_FATAL ANY
    )
    _ohl_render_dependencies_require_archive_manifest(
        "${NEGATIVE_BINARY_DIR}/fixture.zip" allowed.txt
    )
elseif(NEGATIVE_CASE STREQUAL "static-surface")
    _ohl_render_dependencies_validate_sdl_exposed_members(
        SDL3-3.4.10/lib/x64/SDL3_test.lib
    )
else()
    message(FATAL_ERROR "Unknown SDL archive negative case")
endif()
