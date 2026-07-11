function(ohl_prepare_render_dependency_fixture_runtime output_dir)
    set(_source_dir "${OHL_RENDER_DEPS_SOURCE_DIR}/tests/cmake/render_dependencies/fixtures/runtime")
    set(_build_dir "${CMAKE_BINARY_DIR}/synthetic-runtime-build")
    file(REMOVE_RECURSE "${_build_dir}" "${output_dir}")
    file(MAKE_DIRECTORY "${output_dir}")

    set(_configure
        "${CMAKE_COMMAND}"
        -S "${_source_dir}"
        -B "${_build_dir}"
        -G "${CMAKE_GENERATOR}"
        -DCMAKE_BUILD_TYPE=Release
        "-DOHL_FIXTURE_OUTPUT_DIR=${output_dir}"
    )
    if(CMAKE_GENERATOR_PLATFORM)
        list(APPEND _configure -A "${CMAKE_GENERATOR_PLATFORM}")
    endif()
    if(CMAKE_GENERATOR_TOOLSET)
        list(APPEND _configure -T "${CMAKE_GENERATOR_TOOLSET}")
    endif()
    execute_process(
        COMMAND ${_configure}
        RESULT_VARIABLE _configure_result
        OUTPUT_QUIET
        ERROR_VARIABLE _configure_error
    )
    if(NOT _configure_result EQUAL 0)
        message(FATAL_ERROR "Unable to configure synthetic runtime: ${_configure_error}")
    endif()
    execute_process(
        COMMAND "${CMAKE_COMMAND}" --build "${_build_dir}" --config Release
        RESULT_VARIABLE _build_result
        OUTPUT_QUIET
        ERROR_VARIABLE _build_error
    )
    if(NOT _build_result EQUAL 0)
        message(FATAL_ERROR "Unable to build synthetic runtime: ${_build_error}")
    endif()

    if(WIN32)
        set(_sdl_runtime "${output_dir}/SDL3.dll")
        file(GLOB _sdl_implibs
            "${output_dir}/SDL3.lib"
            "${output_dir}/libSDL3.dll.a"
        )
        list(LENGTH _sdl_implibs _implib_count)
        if(NOT _implib_count EQUAL 1)
            message(FATAL_ERROR "Synthetic SDL import-library output is ambiguous")
        endif()
        list(GET _sdl_implibs 0 _sdl_implib)
        set(OHL_RENDER_DEPS_FIXTURE_SDL_IMPLIB "${_sdl_implib}" PARENT_SCOPE)
    elseif(APPLE)
        set(_sdl_runtime "${output_dir}/libSDL3.dylib")
        set(OHL_RENDER_DEPS_MOLTENVK_LIBRARY
            "${output_dir}/libMoltenVK.dylib" PARENT_SCOPE
        )
        set(OHL_RENDER_DEPS_MOLTENVK_LICENSE_FILE
            "${_source_dir}/MOLTENVK_LICENSE.txt" PARENT_SCOPE
        )
    else()
        set(_sdl_runtime "${output_dir}/libSDL3.so")
    endif()
    if(NOT EXISTS "${_sdl_runtime}")
        message(FATAL_ERROR "Synthetic SDL shared runtime was not produced")
    endif()
    set(OHL_RENDER_DEPS_FIXTURE_SDL_RUNTIME "${_sdl_runtime}" PARENT_SCOPE)
    set(OHL_RENDER_DEPS_FIXTURE_SDL_INCLUDE_DIR
        "${OHL_RENDER_DEPS_SOURCE_DIR}/tests/cmake/render_dependencies/fixtures/valid/sdl/include"
        PARENT_SCOPE
    )
endfunction()
