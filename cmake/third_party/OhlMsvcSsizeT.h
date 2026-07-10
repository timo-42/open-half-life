#pragma once

#if defined(_MSC_VER) && !defined(OHL_MSVC_SSIZE_T_DEFINED)
#include <BaseTsd.h>
typedef SSIZE_T ssize_t;
#define OHL_MSVC_SSIZE_T_DEFINED 1
#endif
