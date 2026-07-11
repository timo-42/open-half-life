cmake_minimum_required(VERSION 3.25)

project(ohl_render_dependency_fixture_runtime LANGUAGES CXX)

if(NOT IS_ABSOLUTE "${OHL_FIXTURE_OUTPUT_DIR}")
    message(FATAL_ERROR "OHL_FIXTURE_OUTPUT_DIR must be absolute")
endif()

add_library(SDL3 SHARED sdl_stub.cpp)
set_target_properties(SDL3 PROPERTIES
    ARCHIVE_OUTPUT_DIRECTORY "${OHL_FIXTURE_OUTPUT_DIR}"
    LIBRARY_OUTPUT_DIRECTORY "${OHL_FIXTURE_OUTPUT_DIR}"
    RUNTIME_OUTPUT_DIRECTORY "${OHL_FIXTURE_OUTPUT_DIR}"
)

if(APPLE)
    add_library(MoltenVK SHARED moltenvk_stub.cpp)
    set_target_properties(MoltenVK PROPERTIES
        ARCHIVE_OUTPUT_DIRECTORY "${OHL_FIXTURE_OUTPUT_DIR}"
        LIBRARY_OUTPUT_DIRECTORY "${OHL_FIXTURE_OUTPUT_DIR}"
        RUNTIME_OUTPUT_DIRECTORY "${OHL_FIXTURE_OUTPUT_DIR}"
    )
endif()
