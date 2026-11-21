// Builds without Qt. Exercise the same scale calculation used by the overlay.
#include "../../patches/qtwayland/6.8.2/fractional-output-scale.h"

using PortalWayland::outputScale;
using PortalWayland::scaleDenominator;

// The Pad 3 recovery dialog clipped when the two rounded axes were treated as
// inconsistent: 3392 / 1233 and 2400 / 873 describe the same 275% scale.
static_assert(outputScale(3392, 2400, 1233, 873, 3) == 330);
static_assert(outputScale(2400, 3392, 873, 1233, 3) == 330);
static_assert(outputScale(3392, 2400, 1696, 1200, 2) == 240);
static_assert(outputScale(3392, 2400, 1233, 890, 3) == 360);
static_assert(outputScale(3392, 2400, 1233, 873, 2) == 240);
static_assert(outputScale(20, 20, 10, 10, 2) == 240); // Ambiguous.
static_assert(outputScale(0, 2400, 1233, 873, 3) == 360);

constexpr bool checkPadOutput(int width, int height)
{
    const auto logicalSize = [](int pixels, int scale) {
        return int((2 * scaleDenominator * pixels + scale) / (2 * scale));
    };
    for (int scale = 120; scale <= 600; ++scale) {
        const int integerScale = (scale + 119) / 120;
        if (outputScale(width, height, logicalSize(width, scale), logicalSize(height, scale),
                        integerScale) != scale)
            return false;
    }
    return true;
}

static_assert(checkPadOutput(3392, 2400));
static_assert(checkPadOutput(2400, 3392));
