cmake_minimum_required(VERSION 3.25)

get_filename_component(repository_root "${CMAKE_CURRENT_LIST_DIR}/.." ABSOLUTE)
if(NOT EXISTS "${repository_root}/.git")
  message(STATUS "Tracked-file policy skipped outside a Git checkout")
  return()
endif()
find_program(git_program git REQUIRED)
execute_process(
  COMMAND "${git_program}" -C "${repository_root}" ls-files
  RESULT_VARIABLE git_result
  OUTPUT_VARIABLE tracked_output
  ERROR_VARIABLE git_error
  OUTPUT_STRIP_TRAILING_WHITESPACE
)
if(NOT git_result EQUAL 0)
  message(FATAL_ERROR "git ls-files failed: ${git_error}")
endif()

string(REPLACE "\n" ";" tracked_files "${tracked_output}")
set(prohibited_extensions
  .bin .bsp .cab .cue .dll .exe .hdr .img .iso .mdl .mdf .mds .nrg .pak
  .spr .wad
)
foreach(relative_path IN LISTS tracked_files)
  if(relative_path STREQUAL "")
    continue()
  endif()
  string(TOLOWER "${relative_path}" lower_path)
  if((lower_path MATCHES "^(assets|cache|imported)/") AND
     NOT lower_path STREQUAL "assets/readme.md")
    message(FATAL_ERROR "private media/cache path is tracked: ${relative_path}")
  endif()
  cmake_path(GET lower_path EXTENSION LAST_ONLY extension)
  if(extension IN_LIST prohibited_extensions)
    message(FATAL_ERROR "proprietary-media container is tracked: ${relative_path}")
  endif()

  file(SIZE "${repository_root}/${relative_path}" file_size)
  if(file_size GREATER 52428800)
    message(FATAL_ERROR "unexpected tracked file over 50 MiB: ${relative_path}")
  endif()

  file(READ "${repository_root}/${relative_path}" signature LIMIT 8 HEX)
  string(TOLOWER "${signature}" signature)
  if(signature MATCHES "^(4d5a|4d534346|49574144|50574144|5041434b)")
    message(FATAL_ERROR "proprietary/executable signature is tracked: ${relative_path}")
  endif()
endforeach()

message(STATUS "Tracked-file policy check passed")
