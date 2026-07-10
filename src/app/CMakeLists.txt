add_executable(open-half-life
  main.cpp
)

target_compile_features(open-half-life PRIVATE cxx_std_20)
target_compile_definitions(
  open-half-life
  PRIVATE OHL_VERSION="${PROJECT_VERSION}"
)
target_link_libraries(
  open-half-life
  PRIVATE
    OpenHalfLife::core
    OpenHalfLife::media
    OpenHalfLife::platform
    OpenHalfLife::vfs
)
ohl_enable_warnings(open-half-life)

install(TARGETS open-half-life RUNTIME DESTINATION bin)
