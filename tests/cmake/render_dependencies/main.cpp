#include <SDL3/SDL.h>
#include <volk.h>

#ifndef VK_NO_PROTOTYPES
#error "The render dependency surface must disable Vulkan prototypes"
#endif

#ifndef VOLK_NAMESPACE
#error "volk must be isolated in its C++ namespace"
#endif

static_assert(SDL_MAJOR_VERSION == 3);
static_assert(SDL_MINOR_VERSION == 4);
static_assert(SDL_MICRO_VERSION == 10);
static_assert(VK_API_VERSION_MAJOR(VK_HEADER_VERSION_COMPLETE) == 1);
static_assert(VK_API_VERSION_MINOR(VK_HEADER_VERSION_COMPLETE) == 4);
static_assert(VK_HEADER_VERSION == 350);

int main()
{
    const auto initialize_dispatch = &volk::volkInitializeCustom;
    return initialize_dispatch != nullptr && SDL_GetVersion() == SDL_VERSION ? 0 : 1;
}
