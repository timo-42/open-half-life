execute_process(
    COMMAND "${NINJA_EXECUTABLE}" -C "${BUILD_DIR}" -t commands render_dependency_probe
    RESULT_VARIABLE _result
    OUTPUT_VARIABLE _commands
    ERROR_VARIABLE _error
)
if(NOT _result EQUAL 0)
    message(FATAL_ERROR "Unable to inspect final link commands: ${_error}")
endif()
get_filename_component(_sdl_name "${EXPECTED_SDL_LINK}" NAME)
if(NOT _commands MATCHES "${_sdl_name}")
    message(FATAL_ERROR "Final link does not consume shared SDL: ${_commands}")
endif()
if(NOT _commands MATCHES "ohl_volk_impl")
    message(FATAL_ERROR "Final link does not consume the volk implementation")
endif()
foreach(_platform_library IN LISTS EXPECTED_PLATFORM_LIBS)
    if(_platform_library AND NOT _commands MATCHES "-l${_platform_library}([ \\n]|$)")
        message(FATAL_ERROR
            "Final link is missing transitive platform library ${_platform_library}"
        )
    endif()
endforeach()
if(EXPECTED_MOLTENVK)
    get_filename_component(_moltenvk_name "${EXPECTED_MOLTENVK}" NAME)
    if(NOT _commands MATCHES "${_moltenvk_name}")
        message(FATAL_ERROR "Apple final link is missing external dynamic MoltenVK")
    endif()
endif()
string(TOLOWER "${_commands}" _lower_commands)
if(_lower_commands MATCHES "(^|[ \\t])-lvulkan([ \\t]|$)" OR
   _lower_commands MATCHES "vulkan-1\\.lib" OR
   _lower_commands MATCHES "libvulkan(\\.so|\\.dylib)")
    message(FATAL_ERROR "Final link contains a direct Vulkan loader: ${_commands}")
endif()
