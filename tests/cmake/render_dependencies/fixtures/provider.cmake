macro(ohl_probe_dependency_provider method dependency)
endmacro()

cmake_language(
    SET_DEPENDENCY_PROVIDER ohl_probe_dependency_provider
    SUPPORTED_METHODS FIND_PACKAGE FETCHCONTENT_MAKEAVAILABLE_SERIAL
)
