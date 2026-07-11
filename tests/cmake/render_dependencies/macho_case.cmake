include("${OHL_RENDER_DEPS_SOURCE_DIR}/cmake/RenderDependencies.cmake")

if(NOT EXISTS "${MACHO_FIXTURE_DIR}/${MACHO_CASE}")
    message(FATAL_ERROR "Mach-O fixture is missing: ${MACHO_CASE}")
endif()
_ohl_render_dependencies_validate_moltenvk_dynamic(
    "${MACHO_FIXTURE_DIR}/${MACHO_CASE}"
)
