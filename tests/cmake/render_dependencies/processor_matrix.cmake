include("${OHL_RENDER_DEPS_SOURCE_DIR}/cmake/RenderDependencies.cmake")

foreach(_processor IN ITEMS x86_64 X86_64 amd64 AMD64 x64 X64)
    _ohl_render_dependencies_windows_x64("${_processor}" _accepted)
    if(NOT _accepted)
        message(FATAL_ERROR "Expected x86-64 processor rejection: ${_processor}")
    endif()
endforeach()

foreach(_processor IN ITEMS
    arm64 ARM64 aarch64 AARCH64 arm64ec ARM64EC x86 i686 unknown "AMD64 " ""
)
    _ohl_render_dependencies_windows_x64("${_processor}" _accepted)
    if(_accepted)
        message(FATAL_ERROR "Unexpected Windows processor acceptance: ${_processor}")
    endif()
endforeach()
