if(NOT DEFINED PROGRAM OR NOT DEFINED EXPECTED)
  message(FATAL_ERROR "PROGRAM and EXPECTED are required")
endif()
if(NOT DEFINED ARGUMENTS)
  set(ARGUMENTS --iso does-not-exist.iso)
endif()

execute_process(
  COMMAND "${PROGRAM}" ${ARGUMENTS}
  RESULT_VARIABLE result
  OUTPUT_VARIABLE standard_output
  ERROR_VARIABLE standard_error
)

if(result EQUAL 0)
  message(FATAL_ERROR "command unexpectedly succeeded")
endif()
if(DEFINED EXPECTED_RESULT AND NOT result EQUAL EXPECTED_RESULT)
  message(FATAL_ERROR
    "command returned ${result}, expected ${EXPECTED_RESULT}"
  )
endif()

set(output "${standard_output}${standard_error}")
string(FIND "${output}" "${EXPECTED}" match_position)
if(match_position EQUAL -1)
  message(FATAL_ERROR "expected diagnostic was not found in: ${output}")
endif()
