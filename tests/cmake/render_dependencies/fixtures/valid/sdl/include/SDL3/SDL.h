#pragma once

#define SDL_MAJOR_VERSION 3
#define SDL_MINOR_VERSION 4
#define SDL_MICRO_VERSION 10
#define SDL_VERSION 3004010U

#ifdef __cplusplus
extern "C" {
#endif
unsigned int SDL_GetVersion(void);
#ifdef __cplusplus
}
#endif
