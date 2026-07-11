if(NOT DEFINED PROBE_SOURCE_DIR OR NOT DEFINED PROBE_BINARY_DIR OR
   NOT DEFINED NEGATIVE_CASE OR NOT DEFINED EXPECTED)
    message(FATAL_ERROR "Negative probe arguments are incomplete")
endif()

file(REMOVE_RECURSE "${PROBE_BINARY_DIR}")
set(_command
    "${CMAKE_COMMAND}"
    -S "${PROBE_SOURCE_DIR}"
    -B "${PROBE_BINARY_DIR}"
    -G Ninja
    "-DOHL_RENDER_DEPS_SOURCE_DIR=${OHL_RENDER_DEPS_SOURCE_DIR}"
    "-DOHL_RENDER_DEPS_NEGATIVE_CASE=${NEGATIVE_CASE}"
)
if(NEGATIVE_CASE STREQUAL "offline-real")
    set(_offline_cache "${PROBE_BINARY_DIR}-cache")
    file(REMOVE_RECURSE "${_offline_cache}")
    set(_command
        "${CMAKE_COMMAND}"
        -S "${PROBE_SOURCE_DIR}/real_acquisition"
        -B "${PROBE_BINARY_DIR}"
        -G Ninja
        "-DOHL_RENDER_DEPS_SOURCE_DIR=${OHL_RENDER_DEPS_SOURCE_DIR}"
        "-DOHL_RENDER_DEPS_CACHE_DIR=${_offline_cache}"
        -DOHL_RENDER_DEPS_FETCH=OFF
    )
endif()
if(NEGATIVE_CASE STREQUAL "archive-member-extra" OR
   NEGATIVE_CASE STREQUAL "static-surface")
    set(_command
        "${CMAKE_COMMAND}"
        "-DOHL_RENDER_DEPS_SOURCE_DIR=${OHL_RENDER_DEPS_SOURCE_DIR}"
        "-DNEGATIVE_CASE=${NEGATIVE_CASE}"
        "-DNEGATIVE_BINARY_DIR=${PROBE_BINARY_DIR}"
        -P "${PROBE_SOURCE_DIR}/sdl_archive_negative.cmake"
    )
endif()
if(NEGATIVE_CASE MATCHES "^macho-")
    if(NEGATIVE_CASE STREQUAL "macho-thin-x86")
        set(_macho_file thin-x86_64.dylib)
    elseif(NEGATIVE_CASE STREQUAL "macho-no-arm64")
        set(_macho_file no-arm64.dylib)
    elseif(NEGATIVE_CASE STREQUAL "macho-arm-wrong-type")
        set(_macho_file arm64-wrong-type.dylib)
    elseif(NEGATIVE_CASE STREQUAL "macho-fat-wrong-type")
        set(_macho_file fat-wrong-type.dylib)
    elseif(NEGATIVE_CASE STREQUAL "macho-malformed-count")
        set(_macho_file malformed-count.dylib)
    elseif(NEGATIVE_CASE STREQUAL "macho-truncated-table")
        set(_macho_file truncated-table.dylib)
    elseif(NEGATIVE_CASE STREQUAL "macho-truncated-slice")
        set(_macho_file truncated-slice.dylib)
    elseif(NEGATIVE_CASE STREQUAL "macho-truncated-thin")
        set(_macho_file truncated-thin.dylib)
    elseif(NEGATIVE_CASE STREQUAL "macho-overflow")
        set(_macho_file overflowing-fat64.dylib)
    elseif(NEGATIVE_CASE STREQUAL "macho-overflowing-slice")
        set(_macho_file overflowing-slice.dylib)
    elseif(NEGATIVE_CASE STREQUAL "macho-slice-inside-table")
        set(_macho_file slice-inside-table.dylib)
    elseif(NEGATIVE_CASE STREQUAL "macho-static")
        set(_macho_file static-archive.a)
    else()
        message(FATAL_ERROR "Unknown Mach-O negative case")
    endif()
    set(_command
        "${CMAKE_COMMAND}"
        "-DOHL_RENDER_DEPS_SOURCE_DIR=${OHL_RENDER_DEPS_SOURCE_DIR}"
        "-DMACHO_FIXTURE_DIR=${MACHO_FIXTURE_DIR}"
        "-DMACHO_CASE=${_macho_file}"
        -P "${PROBE_SOURCE_DIR}/macho_case.cmake"
    )
endif()
if(NEGATIVE_CASE STREQUAL "provider")
    list(APPEND _command
        "-DCMAKE_PROJECT_TOP_LEVEL_INCLUDES=${PROBE_SOURCE_DIR}/fixtures/provider.cmake"
    )
endif()

execute_process(
    COMMAND ${_command}
    RESULT_VARIABLE _result
    OUTPUT_VARIABLE _stdout
    ERROR_VARIABLE _stderr
)
set(_output "${_stdout}\n${_stderr}")
if(_result EQUAL 0)
    message(FATAL_ERROR
        "Negative probe '${NEGATIVE_CASE}' unexpectedly configured successfully"
    )
endif()
if(NOT _output MATCHES "${EXPECTED}")
    message(FATAL_ERROR
        "Negative probe '${NEGATIVE_CASE}' did not emit '${EXPECTED}':\n${_output}"
    )
endif()
