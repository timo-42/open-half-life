set(SDL3_VERSION "3.4.10")
set(SDL3_FOUND TRUE)
if(NOT EXISTS "${OHL_RENDER_DEPS_FIXTURE_SDL_RUNTIME}")
    message(FATAL_ERROR "Platform-correct synthetic SDL runtime is required")
endif()
add_library(SDL3::SDL3-shared SHARED IMPORTED)
set_target_properties(SDL3::SDL3-shared PROPERTIES
    IMPORTED_LOCATION "${OHL_RENDER_DEPS_FIXTURE_SDL_RUNTIME}"
    INTERFACE_INCLUDE_DIRECTORIES "${OHL_RENDER_DEPS_FIXTURE_SDL_INCLUDE_DIR}"
)
if(WIN32)
    if(NOT EXISTS "${OHL_RENDER_DEPS_FIXTURE_SDL_IMPLIB}")
        message(FATAL_ERROR "Platform-correct synthetic SDL import library is required")
    endif()
    set_target_properties(SDL3::SDL3-shared PROPERTIES
        IMPORTED_IMPLIB "${OHL_RENDER_DEPS_FIXTURE_SDL_IMPLIB}"
    )
endif()
