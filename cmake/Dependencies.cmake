include(FetchContent)

option(OHL_USE_SYSTEM_UDFREAD "Use an installed libudfread package" OFF)

function(ohl_add_udfread)
  if(TARGET ThirdParty::udfread)
    return()
  endif()

  if(OHL_USE_SYSTEM_UDFREAD)
    find_package(PkgConfig REQUIRED)
    pkg_check_modules(UDFREAD REQUIRED IMPORTED_TARGET libudfread>=1.1.2)
    add_library(ThirdParty::udfread ALIAS PkgConfig::UDFREAD)
    return()
  endif()

  # libudfread uses Meson upstream. Fetch its pinned release and describe the
  # same small static-library target here so consumers need only CMake/Ninja.
  FetchContent_Declare(
    libudfread
    URL https://get.videolan.org/libudfread/libudfread-1.2.0.tar.xz
    URL_HASH
      SHA512=e3ed8dc7fab472ad382b1b6cd068f1dc0084c34e5ec2c460c1dd84fa14a9d368c4d7af08a23efc5057e7f021f5812222c01247d5f304632716c94fc8c689a1a2
    DOWNLOAD_EXTRACT_TIMESTAMP TRUE
    SOURCE_SUBDIR cmake-build-not-provided
  )
  FetchContent_MakeAvailable(libudfread)
  set(
    OHL_UDFREAD_LICENSE_FILE
    "${libudfread_SOURCE_DIR}/COPYING"
    PARENT_SCOPE
  )

  set(udfread_generated_include "${libudfread_BINARY_DIR}/include")
  file(MAKE_DIRECTORY "${udfread_generated_include}/udfread")
  configure_file(
    "${libudfread_SOURCE_DIR}/src/udfread.h"
    "${udfread_generated_include}/udfread/udfread.h"
    COPYONLY
  )
  configure_file(
    "${libudfread_SOURCE_DIR}/src/blockinput.h"
    "${udfread_generated_include}/udfread/blockinput.h"
    COPYONLY
  )

  add_library(ohl_third_party_udfread STATIC
    "${libudfread_SOURCE_DIR}/src/default_blockinput.c"
    "${libudfread_SOURCE_DIR}/src/ecma167.c"
    "${libudfread_SOURCE_DIR}/src/udfread.c"
  )
  add_library(ThirdParty::udfread ALIAS ohl_third_party_udfread)
  set_target_properties(
    ohl_third_party_udfread
    PROPERTIES
      C_STANDARD 99
      C_STANDARD_REQUIRED YES
      C_EXTENSIONS NO
  )
  target_include_directories(
    ohl_third_party_udfread
    SYSTEM
    PUBLIC "${udfread_generated_include}"
    PRIVATE "${libudfread_SOURCE_DIR}/src"
  )
  target_compile_definitions(
    ohl_third_party_udfread
    PRIVATE _ISOC99_SOURCE
  )
  if(MSVC)
    target_compile_options(
      ohl_third_party_udfread
      PRIVATE
        "/FI${CMAKE_CURRENT_FUNCTION_LIST_DIR}/third_party/OhlMsvcSsizeT.h"
    )
  endif()
  if(NOT WIN32)
    target_compile_definitions(
      ohl_third_party_udfread
      PRIVATE
        _POSIX_C_SOURCE=200809L
        HAVE_FCNTL_H=1
        HAVE_PTHREAD_H=1
        HAVE_UNISTD_H=1
    )
    target_link_libraries(ohl_third_party_udfread PRIVATE Threads::Threads)
  endif()
endfunction()

function(ohl_add_unshield)
  if(TARGET ThirdParty::unshield)
    return()
  endif()

  set(ZLIB_BUILD_EXAMPLES OFF CACHE BOOL "" FORCE)
  set(SKIP_INSTALL_ALL ON CACHE BOOL "" FORCE)
  FetchContent_Declare(
    zlib
    GIT_REPOSITORY https://github.com/madler/zlib.git
    GIT_TAG 51b7f2abdade71cd9bb0e7a373ef2610ec6f9daf
    GIT_SHALLOW TRUE
  )
  FetchContent_MakeAvailable(zlib)
  set_target_properties(zlib PROPERTIES EXCLUDE_FROM_ALL TRUE)

  FetchContent_Declare(
    unshield
    GIT_REPOSITORY https://github.com/twogood/unshield.git
    GIT_TAG 51de441ba6893f11026d4671ccef9e8e2a4634fa
    GIT_SHALLOW TRUE
    SOURCE_SUBDIR cmake-build-not-used
  )
  FetchContent_MakeAvailable(unshield)
  set(
    OHL_UNSHIELD_LICENSE_FILE
    "${unshield_SOURCE_DIR}/LICENSE"
    PARENT_SCOPE
  )
  set(
    OHL_ZLIB_LICENSE_FILE
    "${zlib_SOURCE_DIR}/LICENSE"
    PARENT_SCOPE
  )

  set(HAVE_INTTYPES_H 1)
  set(HAVE_MEMORY_H 1)
  set(HAVE_STDBOOL_H 1)
  set(HAVE_STDINT_H 1)
  set(HAVE_STDLIB_H 1)
  set(HAVE_STRING_H 1)
  set(HAVE_SYS_STAT_H 1)
  set(HAVE_SYS_TYPES_H 1)
  if(NOT WIN32)
    set(HAVE_DLFCN_H 1)
    set(HAVE_STRINGS_H 1)
    set(HAVE_UNISTD_H 1)
  endif()
  set(PROJECT_NAME unshield)
  set(PROJECT_VERSION 1.6.2)
  set(SIZE_FORMAT "zu")
  set(USE_OUR_OWN_MD5 1)
  set(unshield_generated_include "${unshield_BINARY_DIR}/include")
  file(MAKE_DIRECTORY "${unshield_generated_include}/lib")
  configure_file(
    "${unshield_SOURCE_DIR}/lib/unshield_config.h.in"
    "${unshield_generated_include}/lib/unshield_config.h"
  )

  add_library(ohl_third_party_unshield STATIC
    "${unshield_SOURCE_DIR}/lib/component.c"
    "${unshield_SOURCE_DIR}/lib/converter.c"
    "${unshield_SOURCE_DIR}/lib/directory.c"
    "${unshield_SOURCE_DIR}/lib/file.c"
    "${unshield_SOURCE_DIR}/lib/file_group.c"
    "${unshield_SOURCE_DIR}/lib/helper.c"
    "${unshield_SOURCE_DIR}/lib/libunshield.c"
    "${unshield_SOURCE_DIR}/lib/log.c"
    "${unshield_SOURCE_DIR}/lib/md5/md5c.c"
  )
  add_library(ThirdParty::unshield ALIAS ohl_third_party_unshield)
  set_target_properties(
    ohl_third_party_unshield
    PROPERTIES
      C_STANDARD 99
      C_STANDARD_REQUIRED YES
      C_EXTENSIONS YES
  )
  target_compile_definitions(
    ohl_third_party_unshield
    PRIVATE
      HAVE_CONFIG_H=1
      PROTOTYPES=1
      UNSHIELD_EXPORT
  )
  if(MSVC)
    target_compile_definitions(
      ohl_third_party_unshield
      PRIVATE _CRT_SECURE_NO_WARNINGS
    )
  endif()
  target_include_directories(
    ohl_third_party_unshield
    SYSTEM
    PUBLIC
      "${unshield_SOURCE_DIR}/lib"
      "$<$<PLATFORM_ID:Windows>:${unshield_SOURCE_DIR}/win32_msvc>"
    PRIVATE
      "${unshield_generated_include}"
      "${unshield_SOURCE_DIR}/lib/md5"
  )
  target_link_libraries(ohl_third_party_unshield PUBLIC zlibstatic)
endfunction()
