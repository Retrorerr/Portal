// SPDX-License-Identifier: LGPL-3.0-only
#pragma once

#include <algorithm>
#include <cstdint>

namespace PortalWayland {

inline constexpr std::int64_t scaleDenominator = 120;

// Recover a unique scale from independently rounded xdg-output dimensions.
// For positive sizes, rounded(pixel / scale) == logical precisely when
// logical - 0.5 <= pixel / scale < logical + 0.5. Keep the bounds in protocol
// units to handle half-pixel ties and avoid floating-point comparisons.
constexpr std::int64_t outputScale(int pixelWidth, int pixelHeight,
                                   int logicalWidth, int logicalHeight, int bufferScale)
{
    const std::int64_t integerScale = bufferScale * scaleDenominator;
    if (bufferScale <= 1 || pixelWidth <= 0 || pixelHeight <= 0
            || logicalWidth <= 0 || logicalHeight <= 0)
        return integerScale;

    const auto first = [](int pixels, int logical) -> std::int64_t {
        return 2 * scaleDenominator * pixels / (2LL * logical + 1) + 1;
    };
    const auto last = [](int pixels, int logical) -> std::int64_t {
        return 2 * scaleDenominator * pixels / (2LL * logical - 1);
    };

    // wl_output advertises the ceiling of the fractional scale as its buffer scale.
    const auto minimum = std::max({integerScale - scaleDenominator + 1,
                                   first(pixelWidth, logicalWidth), first(pixelHeight, logicalHeight)});
    const auto maximum = std::min({integerScale,
                                   last(pixelWidth, logicalWidth), last(pixelHeight, logicalHeight)});
    return minimum == maximum ? minimum : integerScale;
}

} // namespace PortalWayland
