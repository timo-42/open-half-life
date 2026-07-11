include_guard(GLOBAL)

# This module deliberately avoids FetchContent. SDL is consumed only as a
# shared package (or as one audited official binary package on Windows), and
# Vulkan-Headers is reduced to an explicit per-file allowlist.

function(_ohl_render_dependencies_acquire_file url sha256 destination fetch)
    get_filename_component(_destination_dir "${destination}" DIRECTORY)
    file(MAKE_DIRECTORY "${_destination_dir}")

    if(EXISTS "${destination}")
        file(SHA256 "${destination}" _actual_sha256)
        if(NOT _actual_sha256 STREQUAL sha256)
            message(FATAL_ERROR
                "Render dependency cache file has unexpected content: ${destination}"
            )
        endif()
        return()
    endif()

    if(NOT fetch)
        message(FATAL_ERROR
            "OHL_RENDER_DEPS_FETCH=OFF and an audited dependency file is missing: "
            "${destination}"
        )
    endif()

    set(_temporary "${destination}.download")
    file(REMOVE "${_temporary}")
    file(DOWNLOAD
        "${url}"
        "${_temporary}"
        EXPECTED_HASH "SHA256=${sha256}"
        STATUS _download_status
        TLS_VERIFY ON
    )
    list(GET _download_status 0 _download_code)
    list(GET _download_status 1 _download_message)
    if(NOT _download_code EQUAL 0)
        file(REMOVE "${_temporary}")
        message(FATAL_ERROR
            "Failed to acquire audited render dependency ${url}: ${_download_message}"
        )
    endif()
    file(RENAME "${_temporary}" "${destination}")
endfunction()

function(_ohl_render_dependencies_require_hash path sha256)
    if(NOT EXISTS "${path}")
        message(FATAL_ERROR "Required render dependency file is missing: ${path}")
    endif()
    file(SHA256 "${path}" _actual_sha256)
    if(NOT _actual_sha256 STREQUAL sha256)
        message(FATAL_ERROR "Render dependency file hash mismatch: ${path}")
    endif()
endfunction()

function(_ohl_render_dependencies_reject_unlisted_files root label)
    set(_allowed ${ARGN})
    file(GLOB_RECURSE _files LIST_DIRECTORIES FALSE RELATIVE "${root}" "${root}/*")
    foreach(_file IN LISTS _files)
        string(REPLACE "\\" "/" _file "${_file}")
        get_filename_component(_name "${_file}" NAME)
        string(TOLOWER "${_name}" _lower_name)
        if(label STREQUAL "Vulkan" AND
           (_lower_name STREQUAL "vk_icd.h" OR _lower_name STREQUAL "vk_layer.h"))
            message(FATAL_ERROR "Forbidden Vulkan loader header found: ${_file}")
        endif()
        if(label STREQUAL "Vulkan" AND
           _lower_name MATCHES "\\.(zip|tar|tgz|gz|bz2|xz|7z)$")
            message(FATAL_ERROR "A Vulkan-Headers archive is forbidden: ${_file}")
        endif()
        if(NOT _file IN_LIST _allowed)
            message(FATAL_ERROR
                "File is outside the audited ${label} allowlist: ${_file}"
            )
        endif()
    endforeach()
    foreach(_file IN LISTS _allowed)
        if(NOT EXISTS "${root}/${_file}")
            message(FATAL_ERROR "Audited ${label} allowlist file is missing: ${_file}")
        endif()
    endforeach()
endfunction()

function(_ohl_render_dependencies_archive_members archive output)
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -E tar tf "${archive}"
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _listing
        ERROR_VARIABLE _error
    )
    if(NOT _result EQUAL 0)
        message(FATAL_ERROR "Unable to inspect dependency archive: ${_error}")
    endif()
    string(REPLACE "\r" "" _listing "${_listing}")
    string(REPLACE "\n" ";" _members "${_listing}")
    list(FILTER _members EXCLUDE REGEX "^$")
    foreach(_member IN LISTS _members)
        if(IS_ABSOLUTE "${_member}" OR _member MATCHES "(^|/)\\.\\.(/|$)" OR
           _member MATCHES "\\\\")
            message(FATAL_ERROR "Unsafe dependency archive member: ${_member}")
        endif()
    endforeach()
    list(SORT _members)
    set("${output}" "${_members}" PARENT_SCOPE)
endfunction()

function(_ohl_render_dependencies_require_archive_manifest archive)
    set(_expected ${ARGN})
    list(SORT _expected)
    _ohl_render_dependencies_archive_members("${archive}" _actual)
    if(NOT _actual STREQUAL _expected)
        set(_unexpected ${_actual})
        list(REMOVE_ITEM _unexpected ${_expected})
        set(_missing ${_expected})
        list(REMOVE_ITEM _missing ${_actual})
        message(FATAL_ERROR
            "Official SDL archive member manifest mismatch; "
            "unexpected='${_unexpected}', missing='${_missing}'"
        )
    endif()
endfunction()

function(_ohl_render_dependencies_validate_sdl_exposed_members)
    foreach(_member IN LISTS ARGN)
        if(_member MATCHES "(^|/)SDL3_test(\\.|$)" OR
           _member MATCHES "(^|/)lib/(x86|arm64)/" OR
           _member MATCHES "\\.(pdb|a)$")
            message(FATAL_ERROR
                "Static or unrelated SDL archive member cannot be exposed: ${_member}"
            )
        endif()
    endforeach()
endfunction()

function(_ohl_render_dependencies_windows_x64 processor output)
    string(TOLOWER "${processor}" _processor)
    if(_processor STREQUAL "x86_64" OR
       _processor STREQUAL "amd64" OR
       _processor STREQUAL "x64")
        set("${output}" TRUE PARENT_SCOPE)
    else()
        set("${output}" FALSE PARENT_SCOPE)
    endif()
endfunction()

function(_ohl_render_dependencies_read_binary file file_size offset bytes output)
    if(offset LESS 0 OR bytes LESS 1 OR offset GREATER file_size)
        message(FATAL_ERROR "Malformed MoltenVK Mach-O bounds")
    endif()
    math(EXPR _remaining "${file_size} - ${offset}")
    if(bytes GREATER _remaining)
        message(FATAL_ERROR "Truncated MoltenVK Mach-O structure")
    endif()
    file(READ "${file}" _hex OFFSET "${offset}" LIMIT "${bytes}" HEX)
    math(EXPR _expected_length "${bytes} * 2")
    string(LENGTH "${_hex}" _actual_length)
    if(NOT _actual_length EQUAL _expected_length)
        message(FATAL_ERROR "Truncated MoltenVK Mach-O structure")
    endif()
    set("${output}" "${_hex}" PARENT_SCOPE)
endfunction()

function(_ohl_render_dependencies_decode_u32 hex endian output)
    if(endian STREQUAL "LITTLE")
        string(SUBSTRING "${hex}" 0 2 _b0)
        string(SUBSTRING "${hex}" 2 2 _b1)
        string(SUBSTRING "${hex}" 4 2 _b2)
        string(SUBSTRING "${hex}" 6 2 _b3)
        set(_ordered "${_b3}${_b2}${_b1}${_b0}")
    else()
        set(_ordered "${hex}")
    endif()
    math(EXPR _value "0x${_ordered}")
    set("${output}" "${_value}" PARENT_SCOPE)
endfunction()

function(_ohl_render_dependencies_decode_u64 hex endian output)
    if(endian STREQUAL "LITTLE")
        set(_ordered "")
        foreach(_index RANGE 7 0 -1)
            math(EXPR _offset "${_index} * 2")
            string(SUBSTRING "${hex}" "${_offset}" 2 _byte)
            string(APPEND _ordered "${_byte}")
        endforeach()
    else()
        set(_ordered "${hex}")
    endif()
    string(SUBSTRING "${_ordered}" 0 1 _high_nibble)
    if(_high_nibble MATCHES "[89a-fA-F]")
        message(FATAL_ERROR "Overflowing MoltenVK fat Mach-O offset or size")
    endif()
    math(EXPR _value "0x${_ordered}")
    set("${output}" "${_value}" PARENT_SCOPE)
endfunction()

function(
    _ohl_render_dependencies_parse_macho64_slice
    file file_size slice_offset slice_size output_cpu output_type
)
    if(slice_size LESS 32)
        message(FATAL_ERROR "Truncated MoltenVK 64-bit Mach-O slice")
    endif()
    _ohl_render_dependencies_read_binary(
        "${file}" "${file_size}" "${slice_offset}" 4 _magic
    )
    if(_magic STREQUAL "cffaedfe")
        set(_endian LITTLE)
    elseif(_magic STREQUAL "feedfacf")
        set(_endian BIG)
    else()
        message(FATAL_ERROR "MoltenVK slice is not a 64-bit Mach-O image")
    endif()
    math(EXPR _cpu_offset "${slice_offset} + 4")
    math(EXPR _type_offset "${slice_offset} + 12")
    _ohl_render_dependencies_read_binary(
        "${file}" "${file_size}" "${_cpu_offset}" 4 _cpu_hex
    )
    _ohl_render_dependencies_read_binary(
        "${file}" "${file_size}" "${_type_offset}" 4 _type_hex
    )
    _ohl_render_dependencies_decode_u32("${_cpu_hex}" "${_endian}" _cpu)
    _ohl_render_dependencies_decode_u32("${_type_hex}" "${_endian}" _type)
    set("${output_cpu}" "${_cpu}" PARENT_SCOPE)
    set("${output_type}" "${_type}" PARENT_SCOPE)
endfunction()

function(_ohl_render_dependencies_validate_moltenvk_dynamic file)
    file(SIZE "${file}" _file_size)
    if(_file_size LESS 4)
        message(FATAL_ERROR "Truncated MoltenVK Mach-O artifact")
    endif()
    _ohl_render_dependencies_read_binary("${file}" "${_file_size}" 0 4 _magic)

    set(_cpu_type_arm64 16777228)
    set(_mh_dylib 6)
    if(_magic STREQUAL "cffaedfe" OR _magic STREQUAL "feedfacf")
        _ohl_render_dependencies_parse_macho64_slice(
            "${file}" "${_file_size}" 0 "${_file_size}" _cpu _type
        )
        if(NOT _cpu EQUAL _cpu_type_arm64)
            message(FATAL_ERROR "Thin MoltenVK Mach-O is not arm64")
        endif()
        if(NOT _magic STREQUAL "cffaedfe")
            message(FATAL_ERROR "Arm64 MoltenVK Mach-O must be little-endian")
        endif()
        if(NOT _type EQUAL _mh_dylib)
            message(FATAL_ERROR "Arm64 MoltenVK Mach-O is not MH_DYLIB")
        endif()
        return()
    endif()

    if(_magic STREQUAL "cafebabe")
        set(_fat_endian BIG)
        set(_fat64 FALSE)
    elseif(_magic STREQUAL "bebafeca")
        set(_fat_endian LITTLE)
        set(_fat64 FALSE)
    elseif(_magic STREQUAL "cafebabf")
        set(_fat_endian BIG)
        set(_fat64 TRUE)
    elseif(_magic STREQUAL "bfbafeca")
        set(_fat_endian LITTLE)
        set(_fat64 TRUE)
    else()
        message(FATAL_ERROR "MoltenVK artifact is not a supported Mach-O image")
    endif()

    _ohl_render_dependencies_read_binary("${file}" "${_file_size}" 4 4 _count_hex)
    _ohl_render_dependencies_decode_u32("${_count_hex}" "${_fat_endian}" _count)
    if(_count LESS 1 OR _count GREATER 64)
        message(FATAL_ERROR "Malformed MoltenVK fat Mach-O architecture count")
    endif()
    if(_fat64)
        set(_entry_size 32)
    else()
        set(_entry_size 20)
    endif()
    math(EXPR _table_size "8 + ${_count} * ${_entry_size}")
    if(_table_size GREATER _file_size)
        message(FATAL_ERROR "Truncated MoltenVK fat Mach-O table")
    endif()

    set(_has_arm64 FALSE)
    set(_range_starts)
    set(_range_ends)
    math(EXPR _last_index "${_count} - 1")
    foreach(_index RANGE 0 ${_last_index})
        math(EXPR _entry_offset "8 + ${_index} * ${_entry_size}")
        _ohl_render_dependencies_read_binary(
            "${file}" "${_file_size}" "${_entry_offset}" 4 _cpu_hex
        )
        _ohl_render_dependencies_decode_u32("${_cpu_hex}" "${_fat_endian}" _declared_cpu)
        if(_fat64)
            math(EXPR _offset_field "${_entry_offset} + 8")
            math(EXPR _size_field "${_entry_offset} + 16")
            _ohl_render_dependencies_read_binary(
                "${file}" "${_file_size}" "${_offset_field}" 8 _offset_hex
            )
            _ohl_render_dependencies_read_binary(
                "${file}" "${_file_size}" "${_size_field}" 8 _size_hex
            )
            _ohl_render_dependencies_decode_u64(
                "${_offset_hex}" "${_fat_endian}" _slice_offset
            )
            _ohl_render_dependencies_decode_u64(
                "${_size_hex}" "${_fat_endian}" _slice_size
            )
        else()
            math(EXPR _offset_field "${_entry_offset} + 8")
            math(EXPR _size_field "${_entry_offset} + 12")
            _ohl_render_dependencies_read_binary(
                "${file}" "${_file_size}" "${_offset_field}" 4 _offset_hex
            )
            _ohl_render_dependencies_read_binary(
                "${file}" "${_file_size}" "${_size_field}" 4 _size_hex
            )
            _ohl_render_dependencies_decode_u32(
                "${_offset_hex}" "${_fat_endian}" _slice_offset
            )
            _ohl_render_dependencies_decode_u32(
                "${_size_hex}" "${_fat_endian}" _slice_size
            )
        endif()
        if(_slice_size LESS 1 OR _slice_offset LESS _table_size OR
           _slice_offset GREATER _file_size)
            message(FATAL_ERROR "Malformed MoltenVK fat Mach-O slice bounds")
        endif()
        math(EXPR _remaining "${_file_size} - ${_slice_offset}")
        if(_slice_size GREATER _remaining)
            message(FATAL_ERROR "Overflowing MoltenVK fat Mach-O slice")
        endif()
        math(EXPR _slice_end "${_slice_offset} + ${_slice_size}")
        list(LENGTH _range_starts _range_count)
        if(_range_count GREATER 0)
            math(EXPR _last_range "${_range_count} - 1")
            foreach(_range_index RANGE 0 ${_last_range})
                list(GET _range_starts ${_range_index} _old_start)
                list(GET _range_ends ${_range_index} _old_end)
                if(_slice_offset LESS _old_end AND _slice_end GREATER _old_start)
                    message(FATAL_ERROR "Overlapping MoltenVK fat Mach-O slices")
                endif()
            endforeach()
        endif()
        list(APPEND _range_starts "${_slice_offset}")
        list(APPEND _range_ends "${_slice_end}")

        _ohl_render_dependencies_parse_macho64_slice(
            "${file}"
            "${_file_size}"
            "${_slice_offset}"
            "${_slice_size}"
            _slice_cpu
            _slice_type
        )
        if(NOT _slice_cpu EQUAL _declared_cpu)
            message(FATAL_ERROR "MoltenVK fat Mach-O CPU descriptor mismatch")
        endif()
        if(NOT _slice_type EQUAL _mh_dylib)
            message(FATAL_ERROR "MoltenVK fat Mach-O slice is not MH_DYLIB")
        endif()
        if(_slice_cpu EQUAL _cpu_type_arm64)
            _ohl_render_dependencies_read_binary(
                "${file}" "${_file_size}" "${_slice_offset}" 4 _slice_magic
            )
            if(NOT _slice_magic STREQUAL "cffaedfe")
                message(FATAL_ERROR "Arm64 MoltenVK Mach-O must be little-endian")
            endif()
            set(_has_arm64 TRUE)
        endif()
    endforeach()
    if(NOT _has_arm64)
        message(FATAL_ERROR "Universal MoltenVK Mach-O has no arm64 MH_DYLIB slice")
    endif()
endfunction()

function(_ohl_render_dependencies_validate_shared_sdl target)
    if(NOT TARGET "${target}")
        message(FATAL_ERROR "SDL3 package must define the SDL3::SDL3-shared target")
    endif()
    get_target_property(_type "${target}" TYPE)
    if(NOT _type STREQUAL "SHARED_LIBRARY")
        message(FATAL_ERROR
            "SDL3::SDL3-shared must be a shared library target; got ${_type}"
        )
    endif()
    get_target_property(_imported "${target}" IMPORTED)
    if(NOT _imported)
        message(FATAL_ERROR "SDL3::SDL3-shared must be an imported package target")
    endif()

    set(_locations)
    get_target_property(_location "${target}" IMPORTED_LOCATION)
    if(_location AND NOT _location STREQUAL "_location-NOTFOUND")
        list(APPEND _locations "${_location}")
    endif()
    get_target_property(_configurations "${target}" IMPORTED_CONFIGURATIONS)
    foreach(_configuration IN LISTS _configurations)
        string(TOUPPER "${_configuration}" _configuration_upper)
        get_target_property(
            _configuration_location "${target}" "IMPORTED_LOCATION_${_configuration_upper}"
        )
        if(_configuration_location AND
           NOT _configuration_location STREQUAL "_configuration_location-NOTFOUND")
            list(APPEND _locations "${_configuration_location}")
        endif()
    endforeach()
    if(NOT _locations)
        message(FATAL_ERROR "SDL3::SDL3-shared has no imported shared runtime")
    endif()
    foreach(_location IN LISTS _locations)
        if(NOT EXISTS "${_location}")
            message(FATAL_ERROR "SDL3 shared runtime does not exist: ${_location}")
        endif()
        string(TOLOWER "${_location}" _lower_location)
        if(WIN32)
            if(NOT _lower_location MATCHES "\\.dll$")
                message(FATAL_ERROR "SDL3 shared runtime is not a DLL: ${_location}")
            endif()
        elseif(APPLE)
            if(NOT _lower_location MATCHES "(\\.dylib$|\\.framework/[^/]+$)")
                message(FATAL_ERROR "SDL3 shared runtime is not dynamic: ${_location}")
            endif()
        elseif(NOT _lower_location MATCHES "\\.so(\\.[0-9]+)*$")
            message(FATAL_ERROR "SDL3 shared runtime is not a shared object: ${_location}")
        endif()
    endforeach()
    if(WIN32)
        set(_import_libraries)
        get_target_property(_import_library "${target}" IMPORTED_IMPLIB)
        if(_import_library AND NOT _import_library STREQUAL "_import_library-NOTFOUND")
            list(APPEND _import_libraries "${_import_library}")
        endif()
        foreach(_configuration IN LISTS _configurations)
            string(TOUPPER "${_configuration}" _configuration_upper)
            get_target_property(
                _configuration_import_library
                "${target}"
                "IMPORTED_IMPLIB_${_configuration_upper}"
            )
            if(_configuration_import_library AND NOT
               _configuration_import_library STREQUAL
               "_configuration_import_library-NOTFOUND")
                list(APPEND _import_libraries "${_configuration_import_library}")
            endif()
        endforeach()
        if(NOT _import_libraries)
            message(FATAL_ERROR "SDL3 shared package has no import library")
        endif()
        foreach(_import_library IN LISTS _import_libraries)
            if(NOT EXISTS "${_import_library}" OR NOT
               _import_library MATCHES "(\\.lib|\\.dll\\.a)$")
                message(FATAL_ERROR
                    "SDL3 shared package import library is invalid: ${_import_library}"
                )
            endif()
        endforeach()
    endif()
endfunction()

function(_ohl_render_dependencies_load_sdl_config output_version exact_directory)
    set(_find_arguments
        NAMES SDL3Config.cmake sdl3-config.cmake
        PATH_SUFFIXES lib/cmake/SDL3 share/cmake/SDL3 cmake/SDL3 cmake
        NO_CACHE
    )
    if(exact_directory)
        list(APPEND _find_arguments
            PATHS "${exact_directory}"
            NO_DEFAULT_PATH
        )
    elseif(DEFINED SDL3_DIR AND NOT SDL3_DIR STREQUAL "")
        if(NOT IS_ABSOLUTE "${SDL3_DIR}")
            message(FATAL_ERROR "SDL3_DIR must be absolute when provided")
        endif()
        list(APPEND _find_arguments HINTS "${SDL3_DIR}")
    endif()
    find_file(_sdl_config ${_find_arguments})
    if(NOT _sdl_config)
        message(FATAL_ERROR "A system SDL3 CMake config package is required")
    endif()

    get_filename_component(_sdl_config_dir "${_sdl_config}" DIRECTORY)
    find_file(_sdl_version_config
        NAMES SDL3ConfigVersion.cmake sdl3-config-version.cmake
        PATHS "${_sdl_config_dir}"
        NO_DEFAULT_PATH
        NO_CACHE
    )
    if(NOT _sdl_version_config)
        message(FATAL_ERROR "SDL3 package has no config-version file")
    endif()

    set(PACKAGE_FIND_VERSION "3.4")
    set(PACKAGE_FIND_VERSION_MAJOR 3)
    set(PACKAGE_FIND_VERSION_MINOR 4)
    set(PACKAGE_FIND_VERSION_PATCH 0)
    set(PACKAGE_FIND_VERSION_TWEAK 0)
    set(PACKAGE_FIND_VERSION_COUNT 2)
    include("${_sdl_version_config}")
    if(NOT PACKAGE_VERSION_COMPATIBLE OR PACKAGE_VERSION VERSION_LESS "3.4")
        message(FATAL_ERROR "SDL3 package must provide version 3.4 or newer")
    endif()
    set(_package_version "${PACKAGE_VERSION}")

    # Direct config inclusion intentionally bypasses find_package dependency
    # providers. The imported target is still validated independently below.
    include("${_sdl_config}")
    if(DEFINED SDL3_VERSION AND NOT SDL3_VERSION STREQUAL "")
        set(_package_version "${SDL3_VERSION}")
    endif()
    set("${output_version}" "${_package_version}" PARENT_SCOPE)
endfunction()

# Adds the renderer's dependency facade. Including this module has no network
# or target side effects. Calling the function is intentionally single-shot:
# every pre-existing facade target is rejected so partial publication cannot be
# mistaken for a valid dependency set.
function(ohl_add_render_dependencies)
    if(ARGC GREATER 0)
        message(FATAL_ERROR "ohl_add_render_dependencies() does not accept arguments")
    endif()

    set(_published_targets
        ohl_third_party_sdl3
        ohl_third_party_vulkan_headers
        ohl_third_party_volk
        ohl_volk_impl
        ohl_sdl_official_shared
        ohl_third_party_moltenvk_runtime
        ThirdParty::SDL3
        ThirdParty::VulkanHeaders
        ThirdParty::volk
        ThirdParty::MoltenVKRuntime
    )
    foreach(_target IN LISTS _published_targets)
        if(TARGET "${_target}")
            message(FATAL_ERROR
                "Pre-existing render dependency target is forbidden: ${_target}"
            )
        endif()
    endforeach()
    foreach(_target IN ITEMS SDL3::SDL3-shared volk::volk Vulkan::Headers)
        if(TARGET "${_target}")
            message(FATAL_ERROR
                "Pre-existing upstream dependency target is forbidden: ${_target}"
            )
        endif()
    endforeach()

    if(CMAKE_PROJECT_TOP_LEVEL_INCLUDES)
        message(FATAL_ERROR
            "Dependency providers/top-level dependency includes are forbidden for "
            "the render dependency wrapper"
        )
    endif()
    get_cmake_property(_variables VARIABLES)
    foreach(_variable IN LISTS _variables)
        string(TOUPPER "${_variable}" _variable_upper)
        if(_variable_upper MATCHES
           "^FETCHCONTENT_SOURCE_DIR_OHL_(SDL3|VULKAN_HEADERS|VOLK|MOLTENVK)$")
            message(FATAL_ERROR
                "FetchContent source override is forbidden: ${_variable}"
            )
        endif()
    endforeach()

    if(NOT DEFINED OHL_RENDER_DEPS_FETCH)
        set(OHL_RENDER_DEPS_FETCH ON)
    endif()
    if(NOT OHL_RENDER_DEPS_FETCH STREQUAL "ON" AND
       NOT OHL_RENDER_DEPS_FETCH STREQUAL "OFF")
        message(FATAL_ERROR "OHL_RENDER_DEPS_FETCH must be exactly ON or OFF")
    endif()
    if(OHL_RENDER_DEPS_FETCH)
        set(_fetch TRUE)
    else()
        set(_fetch FALSE)
    endif()

    if(NOT DEFINED OHL_RENDER_DEPS_SDL_MODE)
        set(OHL_RENDER_DEPS_SDL_MODE "SYSTEM")
    endif()
    string(TOUPPER "${OHL_RENDER_DEPS_SDL_MODE}" _sdl_mode)
    if(NOT _sdl_mode STREQUAL "SYSTEM" AND
       NOT _sdl_mode STREQUAL "OFFICIAL_BINARY")
        message(FATAL_ERROR
            "OHL_RENDER_DEPS_SDL_MODE must be SYSTEM or OFFICIAL_BINARY"
        )
    endif()

    if(NOT DEFINED OHL_RENDER_DEPS_CACHE_DIR)
        set(OHL_RENDER_DEPS_CACHE_DIR
            "${CMAKE_BINARY_DIR}/_ohl_render_dependencies"
        )
    endif()
    if(NOT IS_ABSOLUTE "${OHL_RENDER_DEPS_CACHE_DIR}")
        message(FATAL_ERROR "OHL_RENDER_DEPS_CACHE_DIR must be absolute")
    endif()

    set(_vulkan_version "1.4.350")
    set(_vulkan_commit "8864cdc896bbc2a9b6eb36b3218fc9ef57908d77")
    set(_volk_version "1.4.350")
    set(_volk_commit "3ca312a4f38baa63d8006b6905abbeeb89c8087d")
    set(_vulkan_base_url
        "https://raw.githubusercontent.com/KhronosGroup/Vulkan-Headers/${_vulkan_commit}"
    )
    set(_volk_base_url
        "https://raw.githubusercontent.com/zeux/volk/${_volk_commit}"
    )

    set(_vulkan_manifest
        "LICENSE.md|ac24e5ea920e4318e4d02c4086ae51f53cfb03feed06c18df1019e7ada1ec7bc"
        "LICENSES/Apache-2.0.txt|cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30"
        "include/vulkan/vk_platform.h|949d517bb83e1d88fd4f1cef02bd3cb9ab50d44e8354cc68227cf2dccfdd3307"
        "include/vulkan/vulkan.h|72b952cc6de70ee12d118d3095e80346b42ff01cfc6f2bbb37ec01800eab1da6"
        "include/vulkan/vulkan_core.h|6d2ba4755774b1d129da6b8e661268b494d2d609df6217c6b6485acf7666b6c2"
        "include/vulkan/vulkan_macos.h|00f532e9b7229488ac9932c2b391d74e9e8566ed91d0dbbad10e15dc356c9293"
        "include/vulkan/vulkan_metal.h|8f84536896b8d3c6c49883ce34242f8922840617d0b184b338c0f83da5bfaaaa"
        "include/vulkan/vulkan_wayland.h|d43ca1c45cd4b2e1119c168b780e2864205ad8119e5f42d9d99aa29d2654a3f2"
        "include/vulkan/vulkan_win32.h|0b11442e7efa0d5b6732b9964edf6dbdf3f6cf1dd58afe7572cc9b7f6de24317"
        "include/vulkan/vulkan_xcb.h|1e1525fc64826ab3ea9e8a6ee6a618ef14db3e27397fcf8fb070bb1ddaf00c0b"
        "include/vulkan/vulkan_xlib.h|82346aaa80a5c984b6a26f55c7c363c65be43029da539ba0fbdfea9840051cca"
        "include/vulkan/vulkan_xlib_xrandr.h|da5d5311bcffa49c614e9f397784e44668466890aeaf4e54c2e0064c20259089"
        "include/vk_video/vulkan_video_codec_av1std_decode.h|f03abf49fcaf2bd179d48d768164f9494264b58401b98899e8860ba297a1e7aa"
        "include/vk_video/vulkan_video_codec_av1std_encode.h|40f84c98d0341246ead3e864e1378df47d861505c5c139da5ed85c352d5e2300"
        "include/vk_video/vulkan_video_codec_av1std.h|9b4ebcef0d6844b226803fa91b4c4c8bf9eb941aa31b19cea48ded0886a8f9e2"
        "include/vk_video/vulkan_video_codec_h264std_decode.h|8c4682860954d6bfc603e200d9e12586bc57265bd37b73a12b1ecf0c1ad42721"
        "include/vk_video/vulkan_video_codec_h264std_encode.h|75cc54e489b1ec7e3c635680cabaf7c9c2ddd4071a485440e5368e76ede30a85"
        "include/vk_video/vulkan_video_codec_h264std.h|f6691f82e4637adda20e56d56951c673cc1ecdf2fa77e09cc8eba4b1f45118c1"
        "include/vk_video/vulkan_video_codec_h265std_decode.h|926a24d94afed2b1ab030e81c3b9e2e6730cca530f7a5a5bd821e7b896b93a41"
        "include/vk_video/vulkan_video_codec_h265std_encode.h|8a56e0c78496affa84eca39ec0f5636df681c04623dd0291aeeadd012df324be"
        "include/vk_video/vulkan_video_codec_h265std.h|01b5f3d0fd9d273a68ed90f898ba94c05c0305280a753bf95fab60d60727c22e"
        "include/vk_video/vulkan_video_codecs_common.h|be6d2495d19e96aca6aa5c11e5c418d1cd72beac23fe19eb9169c06e2843f0af"
        "include/vk_video/vulkan_video_codec_vp9std_decode.h|c3929bbdd0ab79128c7f9bfb9641c9a017b3983529315b771caaafc74fdbb89a"
        "include/vk_video/vulkan_video_codec_vp9std.h|e18e20b197945929d5871836bfde923e38b350b09e1cbe430f1bd802d9eda34e"
    )
    set(_volk_manifest
        "volk.h|479bab725a424d54715fe9e54b1a69b344a9c36fb617c9aa2cc4289814d8acdf"
        "volk.c|698571dde0c43bc296ab5997ffebf5d8176a9d9e586894590069dbae5ace3c68"
        "LICENSE.md|04a0693a84f19e53d281ca98bbb0c86ca77251ab13769c6168e6684feb9a1436"
    )
    set(_provenance_kind "official-pinned-file-allowlist")

    if(OHL_RENDER_DEPS_USE_SYNTHETIC_FIXTURES)
        if(NOT PROJECT_NAME STREQUAL "ohl_render_dependency_probe")
            message(FATAL_ERROR "Synthetic render dependency fixtures are test-only")
        endif()
        if(NOT DEFINED OHL_RENDER_DEPS_SOURCE_DIR OR
           NOT IS_ABSOLUTE "${OHL_RENDER_DEPS_SOURCE_DIR}")
            message(FATAL_ERROR
                "OHL_RENDER_DEPS_SOURCE_DIR must be the absolute repository root"
            )
        endif()
        if(NOT EXISTS "${OHL_RENDER_DEPS_SOURCE_DIR}/cmake/RenderDependencies.cmake")
            message(FATAL_ERROR "OHL_RENDER_DEPS_SOURCE_DIR is not the repository root")
        endif()
        if(NOT DEFINED OHL_RENDER_DEPS_FIXTURE_SET)
            set(OHL_RENDER_DEPS_FIXTURE_SET "valid")
        endif()
        if(NOT OHL_RENDER_DEPS_FIXTURE_SET MATCHES
           "^(valid|static|tampered|shadowed|forbidden|archive|missing)$")
            message(FATAL_ERROR "Unknown synthetic fixture set")
        endif()
        if(NOT _sdl_mode STREQUAL "SYSTEM")
            message(FATAL_ERROR
                "Synthetic fixtures cannot substitute official SDL binary provenance"
            )
        endif()
        set(_fixtures_root
            "${OHL_RENDER_DEPS_SOURCE_DIR}/tests/cmake/render_dependencies/fixtures"
        )
        set(_fixture_root "${_fixtures_root}/valid")
        set(_sdl_fixture_root "${_fixtures_root}/valid/sdl")
        set(_vulkan_root "${_fixtures_root}/valid/vulkan")
        set(_volk_root "${_fixtures_root}/valid/volk")
        if(OHL_RENDER_DEPS_FIXTURE_SET STREQUAL "static")
            set(_sdl_fixture_root "${_fixtures_root}/static/sdl")
        elseif(OHL_RENDER_DEPS_FIXTURE_SET STREQUAL "tampered")
            set(_volk_root "${_fixtures_root}/tampered/volk")
        elseif(OHL_RENDER_DEPS_FIXTURE_SET STREQUAL "shadowed")
            set(_volk_root "${_fixtures_root}/shadowed/volk")
        elseif(OHL_RENDER_DEPS_FIXTURE_SET STREQUAL "forbidden")
            set(_vulkan_root "${_fixtures_root}/forbidden/vulkan")
        elseif(OHL_RENDER_DEPS_FIXTURE_SET STREQUAL "archive")
            set(_vulkan_root "${_fixtures_root}/archive/vulkan")
        elseif(OHL_RENDER_DEPS_FIXTURE_SET STREQUAL "missing")
            set(_volk_root "${_fixtures_root}/missing/volk")
        endif()
        set(_vulkan_manifest
            "LICENSE.md|55562b83950bc41dc8136752cadf0768e1b9a0dea8c124863d2cf87afef23b2d"
            "LICENSES/Apache-2.0.txt|bae23de56d43e72c01bfb1e96b86a6d45736812c915e39f74edbf75180fddc70"
            "include/vulkan/vk_platform.h|e6cf59cea2605338244d7c8dcc059315ecf9a180f983ac95fe9b452a20174c2d"
            "include/vulkan/vulkan.h|2e8298089fcccf88e08e42cae5253732db1e662fd0c55cc7396583b5e2bc7d90"
        )
        set(_volk_manifest
            "volk.h|ec6ac7fde061d61de742177ed173cb4b78b98b810adbc3100bfc74c22a662c71"
            "volk.c|1092fa2664d6388841a121b55d3ebcf5d0ef5d96bbc5cf08c5ca4f186383e043"
            "LICENSE.md|f5c1448dcf6b3177d20706d284aac3f9c7074223ba2d5186ffdb989176ba2d09"
        )
        set(_provenance_kind "synthetic-test-fixture")
    else()
        set(_vulkan_root
            "${OHL_RENDER_DEPS_CACHE_DIR}/vulkan-headers-${_vulkan_commit}"
        )
        set(_volk_root "${OHL_RENDER_DEPS_CACHE_DIR}/volk-${_volk_commit}")
    endif()

    set(_vulkan_allowed_files)
    foreach(_entry IN LISTS _vulkan_manifest)
        string(REPLACE "|" ";" _fields "${_entry}")
        list(GET _fields 0 _relative_path)
        list(GET _fields 1 _sha256)
        list(APPEND _vulkan_allowed_files "${_relative_path}")
        if(OHL_RENDER_DEPS_USE_SYNTHETIC_FIXTURES)
            _ohl_render_dependencies_require_hash(
                "${_vulkan_root}/${_relative_path}" "${_sha256}"
            )
        else()
            _ohl_render_dependencies_acquire_file(
                "${_vulkan_base_url}/${_relative_path}"
                "${_sha256}"
                "${_vulkan_root}/${_relative_path}"
                "${_fetch}"
            )
        endif()
    endforeach()
    _ohl_render_dependencies_reject_unlisted_files(
        "${_vulkan_root}" "Vulkan" ${_vulkan_allowed_files}
    )

    foreach(_entry IN LISTS _volk_manifest)
        string(REPLACE "|" ";" _fields "${_entry}")
        list(GET _fields 0 _relative_path)
        list(GET _fields 1 _sha256)
        if(OHL_RENDER_DEPS_USE_SYNTHETIC_FIXTURES)
            _ohl_render_dependencies_require_hash(
                "${_volk_root}/${_relative_path}" "${_sha256}"
            )
        else()
            _ohl_render_dependencies_acquire_file(
                "${_volk_base_url}/${_relative_path}"
                "${_sha256}"
                "${_volk_root}/${_relative_path}"
                "${_fetch}"
            )
        endif()
    endforeach()
    _ohl_render_dependencies_reject_unlisted_files(
        "${_volk_root}" "volk" volk.h volk.c LICENSE.md
    )

    set(_sdl_target "SDL3::SDL3-shared")
    set(_sdl_provenance "system-package-unpinned")
    set(_sdl_version "")
    set(_sdl_sha256 "")
    set(_sdl_url "")
    if(OHL_RENDER_DEPS_USE_SYNTHETIC_FIXTURES)
        _ohl_render_dependencies_load_sdl_config(
            _sdl_version "${_sdl_fixture_root}/cmake"
        )
        set(_sdl_provenance "synthetic-test-fixture")
    elseif(_sdl_mode STREQUAL "SYSTEM")
        _ohl_render_dependencies_load_sdl_config(_sdl_version "")
    else()
        if(NOT WIN32)
            message(FATAL_ERROR
                "OFFICIAL_BINARY SDL mode currently supports Windows x64 only; "
                "use a system shared SDL3 package on this platform"
            )
        endif()
        _ohl_render_dependencies_windows_x64(
            "${CMAKE_SYSTEM_PROCESSOR}" _windows_x64_processor
        )
        if(NOT CMAKE_SIZEOF_VOID_P EQUAL 8 OR NOT _windows_x64_processor)
            message(FATAL_ERROR
                "The audited official SDL binary package requires an explicit "
                "Windows x86-64 processor spelling (x86_64, AMD64, or x64)"
            )
        endif()
        set(_sdl_version "3.4.10")
        set(_sdl_sha256
            "e2b336b10b037934af98308027410732ef7b22f2c6697d58092aa1c209fae7d7"
        )
        set(_sdl_url
            "https://github.com/libsdl-org/SDL/releases/download/release-${_sdl_version}/SDL3-devel-${_sdl_version}-VC.zip"
        )
        set(_sdl_archive
            "${OHL_RENDER_DEPS_CACHE_DIR}/SDL3-devel-${_sdl_version}-VC.zip"
        )
        _ohl_render_dependencies_acquire_file(
            "${_sdl_url}" "${_sdl_sha256}" "${_sdl_archive}" "${_fetch}"
        )
        set(_sdl_archive_members
            "SDL3-3.4.10/.git-hash"
            "SDL3-3.4.10/INSTALL.md"
            "SDL3-3.4.10/LICENSE.txt"
            "SDL3-3.4.10/README.md"
            "SDL3-3.4.10/cmake/SDL3Config.cmake"
            "SDL3-3.4.10/cmake/SDL3ConfigVersion.cmake"
            "SDL3-3.4.10/cmake/sdlcpu.cmake"
            "SDL3-3.4.10/include/SDL3/SDL.h"
            "SDL3-3.4.10/include/SDL3/SDL_assert.h"
            "SDL3-3.4.10/include/SDL3/SDL_asyncio.h"
            "SDL3-3.4.10/include/SDL3/SDL_atomic.h"
            "SDL3-3.4.10/include/SDL3/SDL_audio.h"
            "SDL3-3.4.10/include/SDL3/SDL_begin_code.h"
            "SDL3-3.4.10/include/SDL3/SDL_bits.h"
            "SDL3-3.4.10/include/SDL3/SDL_blendmode.h"
            "SDL3-3.4.10/include/SDL3/SDL_camera.h"
            "SDL3-3.4.10/include/SDL3/SDL_clipboard.h"
            "SDL3-3.4.10/include/SDL3/SDL_close_code.h"
            "SDL3-3.4.10/include/SDL3/SDL_copying.h"
            "SDL3-3.4.10/include/SDL3/SDL_cpuinfo.h"
            "SDL3-3.4.10/include/SDL3/SDL_dialog.h"
            "SDL3-3.4.10/include/SDL3/SDL_dlopennote.h"
            "SDL3-3.4.10/include/SDL3/SDL_egl.h"
            "SDL3-3.4.10/include/SDL3/SDL_endian.h"
            "SDL3-3.4.10/include/SDL3/SDL_error.h"
            "SDL3-3.4.10/include/SDL3/SDL_events.h"
            "SDL3-3.4.10/include/SDL3/SDL_filesystem.h"
            "SDL3-3.4.10/include/SDL3/SDL_gamepad.h"
            "SDL3-3.4.10/include/SDL3/SDL_gpu.h"
            "SDL3-3.4.10/include/SDL3/SDL_guid.h"
            "SDL3-3.4.10/include/SDL3/SDL_haptic.h"
            "SDL3-3.4.10/include/SDL3/SDL_hidapi.h"
            "SDL3-3.4.10/include/SDL3/SDL_hints.h"
            "SDL3-3.4.10/include/SDL3/SDL_init.h"
            "SDL3-3.4.10/include/SDL3/SDL_intrin.h"
            "SDL3-3.4.10/include/SDL3/SDL_iostream.h"
            "SDL3-3.4.10/include/SDL3/SDL_joystick.h"
            "SDL3-3.4.10/include/SDL3/SDL_keyboard.h"
            "SDL3-3.4.10/include/SDL3/SDL_keycode.h"
            "SDL3-3.4.10/include/SDL3/SDL_loadso.h"
            "SDL3-3.4.10/include/SDL3/SDL_locale.h"
            "SDL3-3.4.10/include/SDL3/SDL_log.h"
            "SDL3-3.4.10/include/SDL3/SDL_main.h"
            "SDL3-3.4.10/include/SDL3/SDL_main_impl.h"
            "SDL3-3.4.10/include/SDL3/SDL_messagebox.h"
            "SDL3-3.4.10/include/SDL3/SDL_metal.h"
            "SDL3-3.4.10/include/SDL3/SDL_misc.h"
            "SDL3-3.4.10/include/SDL3/SDL_mouse.h"
            "SDL3-3.4.10/include/SDL3/SDL_mutex.h"
            "SDL3-3.4.10/include/SDL3/SDL_oldnames.h"
            "SDL3-3.4.10/include/SDL3/SDL_opengl.h"
            "SDL3-3.4.10/include/SDL3/SDL_opengl_glext.h"
            "SDL3-3.4.10/include/SDL3/SDL_opengles.h"
            "SDL3-3.4.10/include/SDL3/SDL_opengles2.h"
            "SDL3-3.4.10/include/SDL3/SDL_opengles2_gl2.h"
            "SDL3-3.4.10/include/SDL3/SDL_opengles2_gl2ext.h"
            "SDL3-3.4.10/include/SDL3/SDL_opengles2_gl2platform.h"
            "SDL3-3.4.10/include/SDL3/SDL_opengles2_khrplatform.h"
            "SDL3-3.4.10/include/SDL3/SDL_pen.h"
            "SDL3-3.4.10/include/SDL3/SDL_pixels.h"
            "SDL3-3.4.10/include/SDL3/SDL_platform.h"
            "SDL3-3.4.10/include/SDL3/SDL_platform_defines.h"
            "SDL3-3.4.10/include/SDL3/SDL_power.h"
            "SDL3-3.4.10/include/SDL3/SDL_process.h"
            "SDL3-3.4.10/include/SDL3/SDL_properties.h"
            "SDL3-3.4.10/include/SDL3/SDL_rect.h"
            "SDL3-3.4.10/include/SDL3/SDL_render.h"
            "SDL3-3.4.10/include/SDL3/SDL_revision.h"
            "SDL3-3.4.10/include/SDL3/SDL_scancode.h"
            "SDL3-3.4.10/include/SDL3/SDL_sensor.h"
            "SDL3-3.4.10/include/SDL3/SDL_stdinc.h"
            "SDL3-3.4.10/include/SDL3/SDL_storage.h"
            "SDL3-3.4.10/include/SDL3/SDL_surface.h"
            "SDL3-3.4.10/include/SDL3/SDL_system.h"
            "SDL3-3.4.10/include/SDL3/SDL_test.h"
            "SDL3-3.4.10/include/SDL3/SDL_test_assert.h"
            "SDL3-3.4.10/include/SDL3/SDL_test_common.h"
            "SDL3-3.4.10/include/SDL3/SDL_test_compare.h"
            "SDL3-3.4.10/include/SDL3/SDL_test_crc32.h"
            "SDL3-3.4.10/include/SDL3/SDL_test_font.h"
            "SDL3-3.4.10/include/SDL3/SDL_test_fuzzer.h"
            "SDL3-3.4.10/include/SDL3/SDL_test_harness.h"
            "SDL3-3.4.10/include/SDL3/SDL_test_log.h"
            "SDL3-3.4.10/include/SDL3/SDL_test_md5.h"
            "SDL3-3.4.10/include/SDL3/SDL_test_memory.h"
            "SDL3-3.4.10/include/SDL3/SDL_thread.h"
            "SDL3-3.4.10/include/SDL3/SDL_time.h"
            "SDL3-3.4.10/include/SDL3/SDL_timer.h"
            "SDL3-3.4.10/include/SDL3/SDL_touch.h"
            "SDL3-3.4.10/include/SDL3/SDL_tray.h"
            "SDL3-3.4.10/include/SDL3/SDL_version.h"
            "SDL3-3.4.10/include/SDL3/SDL_video.h"
            "SDL3-3.4.10/include/SDL3/SDL_vulkan.h"
            "SDL3-3.4.10/lib/arm64/SDL3.dll"
            "SDL3-3.4.10/lib/arm64/SDL3.lib"
            "SDL3-3.4.10/lib/arm64/SDL3.pdb"
            "SDL3-3.4.10/lib/arm64/SDL3_test.lib"
            "SDL3-3.4.10/lib/x64/SDL3.dll"
            "SDL3-3.4.10/lib/x64/SDL3.lib"
            "SDL3-3.4.10/lib/x64/SDL3.pdb"
            "SDL3-3.4.10/lib/x64/SDL3_test.lib"
            "SDL3-3.4.10/lib/x86/SDL3.dll"
            "SDL3-3.4.10/lib/x86/SDL3.lib"
            "SDL3-3.4.10/lib/x86/SDL3.pdb"
            "SDL3-3.4.10/lib/x86/SDL3_test.lib"
        )
        _ohl_render_dependencies_require_archive_manifest(
            "${_sdl_archive}" ${_sdl_archive_members}
        )
        set(_sdl_exposed_manifest
            "SDL3-3.4.10/include/SDL3/SDL_assert.h|f64bd68f3ba36b2d3bf015370c67352fd414bbc26c9d2f7d731f244d3e5f0ecf"
            "SDL3-3.4.10/include/SDL3/SDL_asyncio.h|acec5e4c8cbcfe8dc371adb63ad986ff3521685bca9b683ac4cc18ed13b1e840"
            "SDL3-3.4.10/include/SDL3/SDL_atomic.h|2e12f9943e8f1b7ca3488a75541a15a4ef9fe0a945b6a3f5196578dea46b98db"
            "SDL3-3.4.10/include/SDL3/SDL_audio.h|f2ee08f08ac918f9960005b37200c0dd9c2fa13be3418c0960ea3f87ee13498d"
            "SDL3-3.4.10/include/SDL3/SDL_begin_code.h|00e0196d5356ca8c38557d8d473c78735aacb4412ba0c6d1012e8df387270e28"
            "SDL3-3.4.10/include/SDL3/SDL_bits.h|028c845f51fe13238072778a282514f824d24622f4af0b0af63a3811271d0912"
            "SDL3-3.4.10/include/SDL3/SDL_blendmode.h|3a3e25b43bb2ab520b59a50c101a06f4ce17af27e2250a4dfd6cb296c973c3d1"
            "SDL3-3.4.10/include/SDL3/SDL_camera.h|d0599a5549ecc8ba479427fb8ca9ce44a712b19b473a2e6103399a990153d658"
            "SDL3-3.4.10/include/SDL3/SDL_clipboard.h|6351b0b419168c98f95d2f131bc2f2208fbe0171ab32085488d25adffe8f6e37"
            "SDL3-3.4.10/include/SDL3/SDL_close_code.h|67aeed66c5310ac5dc62bc0f1c03713719f28529033cde6bb668d5964d179c27"
            "SDL3-3.4.10/include/SDL3/SDL_cpuinfo.h|9307452eccae54d8bf35dff79daa2e804e29f244e2208c5e7932dfadcf3e8c12"
            "SDL3-3.4.10/include/SDL3/SDL_dialog.h|b81aaf26316299bd9bfb1017c48a5d4b6fe525c64d0058821b70b3223df579db"
            "SDL3-3.4.10/include/SDL3/SDL_dlopennote.h|10449e075868d279114658d3024300b1e59d42ec98cb2374949ba18478544df4"
            "SDL3-3.4.10/include/SDL3/SDL_endian.h|19405eb59c29e5a4eb69afdcf3c38bd678fcf5660a529ec0b8015a82ba602d91"
            "SDL3-3.4.10/include/SDL3/SDL_error.h|c3fb65e2899d341f84ab3a0bbe99646d51efab252290fff28ce36ffc7b5f9a34"
            "SDL3-3.4.10/include/SDL3/SDL_events.h|bb51b7035b41462b8040ce17677fddc90dd0a1ff3ecd8e97086f474f0be9baa1"
            "SDL3-3.4.10/include/SDL3/SDL_filesystem.h|594df8cacd921611186fe9251c4771bb5a9733d36bdd3e821c2fd9382857097a"
            "SDL3-3.4.10/include/SDL3/SDL_gamepad.h|55350e07e9569d70a25643e578edb53bce60867aebb2a3550b00250033f60a8b"
            "SDL3-3.4.10/include/SDL3/SDL_gpu.h|5ff0919a6bb969100c36a6f92713362b005bcf24bb56bcd7d04ad17b91160efa"
            "SDL3-3.4.10/include/SDL3/SDL_guid.h|556ed320776337ef8af8fada3e3fe4a795dddc2abfb7f4813830acb3272d879e"
            "SDL3-3.4.10/include/SDL3/SDL.h|99cfa6d497d1f2bbf973ad0581333e9895f28737b50d4d0cc2d22cdd40c4d11d"
            "SDL3-3.4.10/include/SDL3/SDL_haptic.h|b44f466ce6ffc44f4390c02c0688b9462ea48c0150373a4187c9c4eaf3829e95"
            "SDL3-3.4.10/include/SDL3/SDL_hidapi.h|129fcc4b99aca44a30dccb0d1c653c1a0b018f3e57d3547c414c81db7de508af"
            "SDL3-3.4.10/include/SDL3/SDL_hints.h|58235988673a629b1ce223c25ab93939597ce41ebb6147badf08fec2bf7dced2"
            "SDL3-3.4.10/include/SDL3/SDL_init.h|54f65b10221de0e1ee59b99d01661a56472bc9abb05690c0d5bd787670965a8d"
            "SDL3-3.4.10/include/SDL3/SDL_iostream.h|81f8719b82f61d00a96dac7b97313115172e82e05361878dbcbaad96e334aed8"
            "SDL3-3.4.10/include/SDL3/SDL_joystick.h|de936f3314f39bb02aaf3afc758aa3af7cf5cd2152c2265369d7dc5abca1f1bb"
            "SDL3-3.4.10/include/SDL3/SDL_keyboard.h|b7664fb34abcf86f669edf4fd5fdef2d58fa025f49d91b4743888f192c519d8d"
            "SDL3-3.4.10/include/SDL3/SDL_keycode.h|82c54f5b995c952d6bda3dae6a409d0115b604ee17da3c77de8fe62dbb9c8e60"
            "SDL3-3.4.10/include/SDL3/SDL_loadso.h|14246190e220d2ec52c40a586ceb571ee928215d09e1d82b260675da3a94634f"
            "SDL3-3.4.10/include/SDL3/SDL_locale.h|a65538f0a78a6fc5477c41a0ef359c8b57eacf03c2998d48cac48dd7645bd6f8"
            "SDL3-3.4.10/include/SDL3/SDL_log.h|15f28418f4fe636b41f78541bd73217c32ec0e11a2ef0f39c2ba8033fc3702cf"
            "SDL3-3.4.10/include/SDL3/SDL_messagebox.h|124139156dc7887b3f6fc5f8dfd45b9711d4d834204baa20dec89aebba522918"
            "SDL3-3.4.10/include/SDL3/SDL_metal.h|0689f189d73dc0b9b314013f77505cbeca10d96b23556a057ee8d676b46d8983"
            "SDL3-3.4.10/include/SDL3/SDL_misc.h|3302ddfec11a0f2a1d9e0eda88f6f23ca189cf93d401a01ca826bfcad160e6ec"
            "SDL3-3.4.10/include/SDL3/SDL_mouse.h|6caff19a4841afc5cab4254e94a73de874a331a6539f87d2d2c61a7c5e5d7e5b"
            "SDL3-3.4.10/include/SDL3/SDL_mutex.h|c0218aeee31ba40f4f2990dfff5184d22cdd39907da834027d31f75f3a50a32e"
            "SDL3-3.4.10/include/SDL3/SDL_oldnames.h|59cb5dde7a441d0b208c52f12f73a949dd747ce6b053f31b356cb64cef20e6dc"
            "SDL3-3.4.10/include/SDL3/SDL_pen.h|30cf6dacd1a7ce079c4c2e53df32f85ecdf1f4056fce08089abc26208bb32055"
            "SDL3-3.4.10/include/SDL3/SDL_pixels.h|a4c17ec0b7bd9ddd631cad62cb38bae7d3e1f2b9522914a816a5e86433691ade"
            "SDL3-3.4.10/include/SDL3/SDL_platform_defines.h|ab5692c00d4aca81466688a6c9abe7f5154070b6118f48a67490d81dff6ad36e"
            "SDL3-3.4.10/include/SDL3/SDL_platform.h|9a95a10a7f3c3d8f4958ebaded5c5e3b154f75c7351b1002e32429bc7dd28501"
            "SDL3-3.4.10/include/SDL3/SDL_power.h|ed0346c3c7454da4e305feb5a650a0859053442c68b388dc60848e002b86f3e2"
            "SDL3-3.4.10/include/SDL3/SDL_process.h|a52b5af99b1130189c3510269ba73d8288b55feb4b7086a1614cfad395794655"
            "SDL3-3.4.10/include/SDL3/SDL_properties.h|f66e621b2b9d7d9c8126bfbc31a50f90f515ddf9e090b6e1cc6ea24b343bdd4a"
            "SDL3-3.4.10/include/SDL3/SDL_rect.h|f5a5c39fbffd41eadbeb50e2ddfa4ffe655ff14241a23e4f20de4cce48e82d6e"
            "SDL3-3.4.10/include/SDL3/SDL_render.h|29b5c4a477bf5d20043ed58cd3d32cc71d2ea435edde85cbfeeee94d608d626f"
            "SDL3-3.4.10/include/SDL3/SDL_scancode.h|d5e3bb25b523b61ed354189919f2bcb058f0f2fa08e0529ce8e21480937c8ca5"
            "SDL3-3.4.10/include/SDL3/SDL_sensor.h|8c91002fc95799b820d07671541c97c012f77bcf8eb16f624e717142fa051c72"
            "SDL3-3.4.10/include/SDL3/SDL_stdinc.h|e0d5fca5b28872c2bb67daed0d4c3ad38d349a5a790b85193f02b917f941ce35"
            "SDL3-3.4.10/include/SDL3/SDL_storage.h|fbb7342d44526e0bb683fb645910eb1d3fcffd1468b9c823b0654ef424ba60f0"
            "SDL3-3.4.10/include/SDL3/SDL_surface.h|8c4c6f4127b663b38167be0e431a6015533d9b792f8df42e213b8ec9a1d3ba96"
            "SDL3-3.4.10/include/SDL3/SDL_system.h|81d78f7a087bb1d122565b2310ad139a3672d372e3d1c4a615b00c2403edeb90"
            "SDL3-3.4.10/include/SDL3/SDL_thread.h|b5f2b2d88729338cb0c1e95398f112bccbb343fcf960e2dae080e3695be1be76"
            "SDL3-3.4.10/include/SDL3/SDL_time.h|db6dd02434f0b099748e728d1145486888659087e7742f7367da9e4d1120c771"
            "SDL3-3.4.10/include/SDL3/SDL_timer.h|60a809df30e0c549a91b986f517ff94515ab80a529db95d97655a2c417f0f927"
            "SDL3-3.4.10/include/SDL3/SDL_touch.h|e69c97c2fc210f1e8eab7199173097b4f4196a3ea4a39e1d08a48a145cc92132"
            "SDL3-3.4.10/include/SDL3/SDL_tray.h|00fe9d97cef15acc3adb18a85689a619178c74df12990090da475c4f7e0e12a1"
            "SDL3-3.4.10/include/SDL3/SDL_version.h|ed082eea3bc472409d4bca0fc4e0d8fe50c969f5fd91717844f20480b1d4932c"
            "SDL3-3.4.10/include/SDL3/SDL_video.h|e4a25b506dfb5b0657edc264b7130cfea3bad0abc0265e280e046f9ec4dcaeaf"
            "SDL3-3.4.10/include/SDL3/SDL_vulkan.h|a0d1b8c6a2a7982e419dbdb8ea60fce09e99390238634ca117d82442953a4e1c"
            "SDL3-3.4.10/lib/x64/SDL3.dll|c39fbda24eca1009b06a4d4e340d12511e3c8b0d44c4898d29e336e2cc7a25f0"
            "SDL3-3.4.10/lib/x64/SDL3.lib|04c41f986a80243d1048ed026c0a145515140c99f8c895710083c3da27d4c03a"
            "SDL3-3.4.10/LICENSE.txt|1c040b8271b37e5076359f8fd54240e371114112924d2df81ef87c7d6a1dfdfd"
        )
        set(_sdl_exposed_members)
        foreach(_entry IN LISTS _sdl_exposed_manifest)
            string(REPLACE "|" ";" _fields "${_entry}")
            list(GET _fields 0 _member)
            list(APPEND _sdl_exposed_members "${_member}")
        endforeach()
        _ohl_render_dependencies_validate_sdl_exposed_members(
            ${_sdl_exposed_members}
        )
        set(_sdl_extract_root "${CMAKE_BINARY_DIR}/_ohl_sdl_official_binary")
        file(REMOVE_RECURSE "${_sdl_extract_root}")
        file(MAKE_DIRECTORY "${_sdl_extract_root}")
        execute_process(
            COMMAND "${CMAKE_COMMAND}" -E tar xvf
                "${_sdl_archive}" ${_sdl_exposed_members}
            WORKING_DIRECTORY "${_sdl_extract_root}"
            RESULT_VARIABLE _sdl_extract_result
            ERROR_VARIABLE _sdl_extract_error
            OUTPUT_QUIET
        )
        if(NOT _sdl_extract_result EQUAL 0)
            message(FATAL_ERROR
                "Unable to extract audited SDL members: ${_sdl_extract_error}"
            )
        endif()
        foreach(_entry IN LISTS _sdl_exposed_manifest)
            string(REPLACE "|" ";" _fields "${_entry}")
            list(GET _fields 0 _member)
            list(GET _fields 1 _member_sha256)
            _ohl_render_dependencies_require_hash(
                "${_sdl_extract_root}/${_member}" "${_member_sha256}"
            )
        endforeach()
        _ohl_render_dependencies_reject_unlisted_files(
            "${_sdl_extract_root}" "SDL" ${_sdl_exposed_members}
        )
        set(_sdl_prefix "${_sdl_extract_root}/SDL3-${_sdl_version}")
        set(_sdl_runtime "${_sdl_prefix}/lib/x64/SDL3.dll")
        set(_sdl_import_library "${_sdl_prefix}/lib/x64/SDL3.lib")
        set(_sdl_include_dir "${_sdl_prefix}/include")
        set(_sdl_license "${_sdl_prefix}/LICENSE.txt")
        foreach(_path IN ITEMS
            "${_sdl_runtime}"
            "${_sdl_import_library}"
            "${_sdl_include_dir}/SDL3/SDL.h"
            "${_sdl_license}"
        )
            if(NOT EXISTS "${_path}")
                message(FATAL_ERROR "Official SDL binary package is incomplete: ${_path}")
            endif()
        endforeach()
        file(GLOB_RECURSE _sdl_source_files LIST_DIRECTORIES FALSE
            "${_sdl_prefix}/*.c"
            "${_sdl_prefix}/*.cc"
            "${_sdl_prefix}/*.cpp"
            "${_sdl_prefix}/*.cxx"
            "${_sdl_prefix}/*.m"
            "${_sdl_prefix}/*.mm"
        )
        if(_sdl_source_files)
            message(FATAL_ERROR "Official SDL binary package unexpectedly contains source")
        endif()
        set(_sdl_target "ohl_sdl_official_shared")
        set(_sdl_provenance "official-release-binary")
    endif()

    if(NOT _sdl_mode STREQUAL "OFFICIAL_BINARY" OR
       OHL_RENDER_DEPS_USE_SYNTHETIC_FIXTURES)
        _ohl_render_dependencies_validate_shared_sdl("SDL3::SDL3-shared")
    endif()

    if(APPLE)
        if(DEFINED OHL_RENDER_DEPS_MOLTENVK_STATIC_LIBRARY)
            message(FATAL_ERROR "A static MoltenVK fallback is forbidden")
        endif()
        foreach(_variable IN ITEMS
            OHL_RENDER_DEPS_MOLTENVK_LIBRARY
            OHL_RENDER_DEPS_MOLTENVK_LICENSE_FILE
        )
            if(NOT DEFINED ${_variable} OR NOT IS_ABSOLUTE "${${_variable}}")
                message(FATAL_ERROR
                    "${_variable} must name an absolute pre-provisioned external file"
                )
            endif()
            if(NOT EXISTS "${${_variable}}" OR IS_DIRECTORY "${${_variable}}")
                message(FATAL_ERROR "External MoltenVK artifact is missing: ${${_variable}}")
            endif()
        endforeach()
        set(_moltenvk_binary "${OHL_RENDER_DEPS_MOLTENVK_LIBRARY}")
        if(_moltenvk_binary MATCHES "\\.framework/[^/]+$")
            set(_moltenvk_runtime_kind "external-dynamic-framework")
            set(_moltenvk_is_framework TRUE)
        elseif(_moltenvk_binary MATCHES "\\.dylib$")
            set(_moltenvk_runtime_kind "external-dynamic-dylib")
            set(_moltenvk_is_framework FALSE)
        else()
            message(FATAL_ERROR
                "External MoltenVK must be a dynamic .dylib or framework binary"
            )
        endif()
        _ohl_render_dependencies_validate_moltenvk_dynamic("${_moltenvk_binary}")
    endif()

    # All fallible policy and content validation above completes before the
    # project-owned dependency facade is published.
    if(_sdl_mode STREQUAL "OFFICIAL_BINARY" AND
       NOT OHL_RENDER_DEPS_USE_SYNTHETIC_FIXTURES)
        add_library(ohl_sdl_official_shared SHARED IMPORTED GLOBAL)
        set_target_properties(ohl_sdl_official_shared PROPERTIES
            IMPORTED_LOCATION "${_sdl_runtime}"
            IMPORTED_IMPLIB "${_sdl_import_library}"
            INTERFACE_INCLUDE_DIRECTORIES "${_sdl_include_dir}"
        )
    endif()

    add_library(ohl_volk_impl STATIC "${_volk_root}/volk.c")
    set_source_files_properties("${_volk_root}/volk.c" PROPERTIES LANGUAGE CXX)
    target_compile_features(ohl_volk_impl PUBLIC cxx_std_20)
    target_compile_definitions(ohl_volk_impl PUBLIC VK_NO_PROTOTYPES VOLK_NAMESPACE)
    target_include_directories(ohl_volk_impl SYSTEM PUBLIC
        "${_vulkan_root}/include"
        "${_volk_root}"
    )
    if(NOT WIN32)
        target_link_libraries(ohl_volk_impl PUBLIC "${CMAKE_DL_LIBS}")
    endif()

    add_library(ohl_third_party_sdl3 INTERFACE)
    add_library(ThirdParty::SDL3 ALIAS ohl_third_party_sdl3)
    target_link_libraries(ohl_third_party_sdl3 INTERFACE "${_sdl_target}")
    set_target_properties(ohl_third_party_sdl3 PROPERTIES
        OHL_PROVENANCE_KIND "${_sdl_provenance}"
        OHL_PACKAGE_VERSION "${_sdl_version}"
        OHL_LINKAGE "shared"
        OHL_VULKAN_LOADER_SOURCE "SDL_Vulkan_GetVkGetInstanceProcAddr"
    )
    if(_sdl_provenance STREQUAL "official-release-binary")
        set_target_properties(ohl_third_party_sdl3 PROPERTIES
            OHL_PINNED_VERSION "${_sdl_version}"
            OHL_PINNED_SHA256 "${_sdl_sha256}"
            OHL_SOURCE_URL "${_sdl_url}"
        )
    endif()

    add_library(ohl_third_party_vulkan_headers INTERFACE)
    add_library(ThirdParty::VulkanHeaders ALIAS ohl_third_party_vulkan_headers)
    target_include_directories(ohl_third_party_vulkan_headers SYSTEM INTERFACE
        "${_vulkan_root}/include"
    )
    set_target_properties(ohl_third_party_vulkan_headers PROPERTIES
        OHL_PROVENANCE_KIND "${_provenance_kind}"
        OHL_ACQUISITION_KIND "individual-file-allowlist"
        OHL_VULKAN_API_BASELINE "1.1"
    )
    if(OHL_RENDER_DEPS_USE_SYNTHETIC_FIXTURES)
        set_target_properties(ohl_third_party_vulkan_headers PROPERTIES
            OHL_PRODUCTION_PIN_COMMIT "${_vulkan_commit}"
        )
    else()
        set_target_properties(ohl_third_party_vulkan_headers PROPERTIES
            OHL_PINNED_VERSION "${_vulkan_version}"
            OHL_PINNED_COMMIT "${_vulkan_commit}"
            OHL_SOURCE_URL "${_vulkan_base_url}"
        )
    endif()

    add_library(ohl_third_party_volk INTERFACE)
    add_library(ThirdParty::volk ALIAS ohl_third_party_volk)
    target_link_libraries(ohl_third_party_volk INTERFACE ohl_volk_impl)
    set_target_properties(ohl_third_party_volk PROPERTIES
        OHL_PROVENANCE_KIND "${_provenance_kind}"
        OHL_ACQUISITION_KIND "individual-file-allowlist"
        OHL_INITIALIZATION_ENTRYPOINT "volkInitializeCustom"
        OHL_DIRECT_VULKAN_LOADER_LINK "FALSE"
    )
    if(OHL_RENDER_DEPS_USE_SYNTHETIC_FIXTURES)
        set_target_properties(ohl_third_party_volk PROPERTIES
            OHL_PRODUCTION_PIN_COMMIT "${_volk_commit}"
        )
    else()
        set_target_properties(ohl_third_party_volk PROPERTIES
            OHL_PINNED_VERSION "${_volk_version}"
            OHL_PINNED_COMMIT "${_volk_commit}"
            OHL_SOURCE_URL "${_volk_base_url}"
        )
    endif()

    if(APPLE)
        add_library(ohl_third_party_moltenvk_runtime SHARED IMPORTED GLOBAL)
        add_library(
            ThirdParty::MoltenVKRuntime ALIAS ohl_third_party_moltenvk_runtime
        )
        set_target_properties(ohl_third_party_moltenvk_runtime PROPERTIES
            IMPORTED_LOCATION "${_moltenvk_binary}"
            FRAMEWORK "${_moltenvk_is_framework}"
            OHL_PROVENANCE_KIND "unpinned-external-dynamic"
            OHL_RUNTIME_KIND "${_moltenvk_runtime_kind}"
            OHL_RUNTIME_BINARY "${_moltenvk_binary}"
            OHL_RUNTIME_RPATH "@rpath"
            OHL_PORTABILITY_ENUMERATION_REQUIRED TRUE
        )
        set(OHL_MOLTENVK_LICENSE_FILE
            "${OHL_RENDER_DEPS_MOLTENVK_LICENSE_FILE}" PARENT_SCOPE
        )
    endif()

    if(_sdl_provenance STREQUAL "official-release-binary")
        set(OHL_SDL3_LICENSE_FILE "${_sdl_license}" PARENT_SCOPE)
    endif()
    set(OHL_VULKAN_HEADERS_LICENSE_FILE
        "${_vulkan_root}/LICENSE.md" PARENT_SCOPE
    )
    set(OHL_VULKAN_HEADERS_LICENSE_FILES
        "${_vulkan_root}/LICENSE.md;${_vulkan_root}/LICENSES/Apache-2.0.txt"
        PARENT_SCOPE
    )
    set(OHL_VOLK_LICENSE_FILE "${_volk_root}/LICENSE.md" PARENT_SCOPE)
endfunction()
