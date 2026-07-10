#pragma once

// libudfread's Meson configuration normally provides these MSVC adaptations.
// Our pinned-source CMake target force-includes this private compatibility
// header instead, without changing or exposing definitions from upstream.
#if defined(_MSC_VER)
#include <BaseTsd.h>
#include <fcntl.h>

#if !defined(OHL_MSVC_SSIZE_T_DEFINED)
typedef SSIZE_T ssize_t;
#define OHL_MSVC_SSIZE_T_DEFINED 1
#endif

#if !defined(O_RDONLY)
#define O_RDONLY _O_RDONLY
#endif

#if !defined(O_BINARY)
#define O_BINARY _O_BINARY
#endif
#endif
