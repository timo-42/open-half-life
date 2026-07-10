if(NOT DEFINED PROGRAM OR NOT DEFINED EXPECTED)
  message(FATAL_ERROR "PROGRAM and EXPECTED are required")
endif()

execute_process(
  COMMAND "${PROGRAM}" --iso does-not-exist.iso
  RESULT_VARIABLE result
  OUTPUT_VARIABLE standard_output
  ERROR_VARIABLE standard_error
)

if(result EQUAL 0)
  message(FATAL_ERROR "command unexpectedly succeeded")
endif()

set(output "${standard_output}${standard_error}")
string(FIND "${output}" "${EXPECTED}" match_position)
if(match_position EQUAL -1)
  message(FATAL_ERROR "expected diagnostic was not found in: ${output}")
endif()
