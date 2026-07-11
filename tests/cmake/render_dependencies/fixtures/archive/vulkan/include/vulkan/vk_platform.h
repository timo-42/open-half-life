#pragma once

#define VK_MAKE_API_VERSION(variant, major, minor, patch) \
    (((variant) << 29U) | ((major) << 22U) | ((minor) << 12U) | (patch))
#define VK_API_VERSION_MAJOR(version) (((version) >> 22U) & 0x7FU)
#define VK_API_VERSION_MINOR(version) (((version) >> 12U) & 0x3FFU)
