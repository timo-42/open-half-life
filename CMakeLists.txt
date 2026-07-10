cmake_minimum_required(VERSION 3.25)

project(
  OpenHalfLife
  VERSION 0.2.0
  DESCRIPTION "Clean-room, cross-platform Half-Life runtime reimplementation"
  LANGUAGES C CXX
)

option(OHL_WARNINGS_AS_ERRORS "Treat compiler warnings as errors" OFF)
option(
  OHL_ENABLE_EXPERIMENTAL_INSTALLSHIELD
  "Build the unaudited InstallShield cabinet adapter"
  OFF
)
set(CMAKE_CXX_EXTENSIONS OFF)

list(APPEND CMAKE_MODULE_PATH "${CMAKE_CURRENT_SOURCE_DIR}/cmake")
include(OhlWarnings)
include(Dependencies)

find_package(Threads REQUIRED)
ohl_add_udfread()
if(OHL_ENABLE_EXPERIMENTAL_INSTALLSHIELD)
  ohl_add_unshield()
endif()

add_subdirectory(src/core)
add_subdirectory(src/platform)
add_subdirectory(src/vfs)
add_subdirectory(src/media)
add_subdirectory(src/app)

include(CTest)
if(BUILD_TESTING)
  add_subdirectory(tests)
endif()

install(
  FILES
    LICENSE
    THIRD_PARTY_NOTICES.md
    TRADEMARKS.md
  DESTINATION share/doc/open-half-life
)
if(DEFINED OHL_UDFREAD_LICENSE_FILE)
  install(
    FILES "${OHL_UDFREAD_LICENSE_FILE}"
    DESTINATION share/doc/open-half-life
    RENAME LICENSE.libudfread
  )
endif()
if(DEFINED OHL_UNSHIELD_LICENSE_FILE)
  install(
    FILES "${OHL_UNSHIELD_LICENSE_FILE}"
    DESTINATION share/doc/open-half-life
    RENAME LICENSE.unshield
  )
  install(
    FILES "${OHL_ZLIB_LICENSE_FILE}"
    DESTINATION share/doc/open-half-life
    RENAME LICENSE.zlib
  )
endif()
