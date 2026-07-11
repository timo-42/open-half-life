if(NOT DEFINED OHL_INSTALL_SMOKE_PREFIX)
  message(FATAL_ERROR "OHL_INSTALL_SMOKE_PREFIX is required")
endif()
if(NOT DEFINED OHL_INSTALL_SMOKE_BUILD_DIR)
  message(FATAL_ERROR "OHL_INSTALL_SMOKE_BUILD_DIR is required")
endif()
if(NOT DEFINED OHL_INSTALL_SMOKE_EXECUTABLE)
  message(FATAL_ERROR "OHL_INSTALL_SMOKE_EXECUTABLE is required")
endif()

file(REMOVE_RECURSE "${OHL_INSTALL_SMOKE_PREFIX}")
execute_process(
  COMMAND
    "${CMAKE_COMMAND}"
    --install "${OHL_INSTALL_SMOKE_BUILD_DIR}"
    --prefix "${OHL_INSTALL_SMOKE_PREFIX}"
    --component media_parser_worker
  RESULT_VARIABLE install_result
)
if(NOT install_result EQUAL 0)
  message(FATAL_ERROR "install failed with ${install_result}")
endif()

set(
  installed_worker
  "${OHL_INSTALL_SMOKE_PREFIX}/libexec/open-half-life/ohl-media-parser-worker"
)
execute_process(
  COMMAND "${OHL_INSTALL_SMOKE_EXECUTABLE}" "${installed_worker}"
  RESULT_VARIABLE smoke_result
)
if(NOT smoke_result EQUAL 0)
  message(FATAL_ERROR "installed worker smoke failed with ${smoke_result}")
endif()
