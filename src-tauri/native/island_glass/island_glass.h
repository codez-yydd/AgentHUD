#pragma once

#include <cstdint>

struct AgentHudGlassSurface {
    float x, y, width, height;
    float radii[4];
};

extern "C" {
void* agenthud_glass_start(std::intptr_t anchor) noexcept;
void agenthud_glass_stop(void* handle) noexcept;
void agenthud_glass_set_surface(void* handle, const AgentHudGlassSurface* surface) noexcept;
void agenthud_glass_set_paused(void* handle, int paused) noexcept;
int agenthud_glass_is_ready(const void* handle) noexcept;
int agenthud_glass_luma(const void* handle) noexcept;
}
