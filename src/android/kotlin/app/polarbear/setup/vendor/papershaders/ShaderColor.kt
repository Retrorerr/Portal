package app.polarbear.setup.vendor.papershaders

// Vendored from AndreFrelicot/paper-shaders-android, tag 0.0.3
// (Apache License 2.0, https://github.com/AndreFrelicot/paper-shaders-android):
//   paper-shaders-compose/src/main/kotlin/dev/andrefrelicot/papershaders/ShaderColor.kt
// Unmodified except for the Kotlin package declaration.

/** Parsed straight RGBA color components in 0..1 float space. */
public data class ShaderColor(
    val r: Float,
    val g: Float,
    val b: Float,
    val a: Float = 1f,
) {
    /** Components ordered for `RuntimeShader.setFloatUniform(name, r, g, b, a)`. */
    public val components: FloatArray
        get() = floatArrayOf(r, g, b, a)

    public companion object {
        /** Parses `#rgb`, `#rrggbb`, or `#rrggbbaa`; invalid input becomes opaque black. */
        public fun parse(value: String): ShaderColor {
            val trimmed = value.trim()
            if (!trimmed.startsWith("#")) return ShaderColor(0f, 0f, 0f, 1f)

            val hex = trimmed.drop(1)
            return when (hex.length) {
                3 -> fromHex("${hex[0]}${hex[0]}${hex[1]}${hex[1]}${hex[2]}${hex[2]}")
                6, 8 -> fromHex(hex)
                else -> ShaderColor(0f, 0f, 0f, 1f)
            }
        }

        private fun fromHex(hex: String): ShaderColor {
            val value = hex.toLongOrNull(radix = 16) ?: return ShaderColor(0f, 0f, 0f, 1f)
            return if (hex.length == 8) {
                ShaderColor(
                    r = ((value shr 24) and 0xff) / 255f,
                    g = ((value shr 16) and 0xff) / 255f,
                    b = ((value shr 8) and 0xff) / 255f,
                    a = (value and 0xff) / 255f,
                )
            } else {
                ShaderColor(
                    r = ((value shr 16) and 0xff) / 255f,
                    g = ((value shr 8) and 0xff) / 255f,
                    b = (value and 0xff) / 255f,
                    a = 1f,
                )
            }
        }
    }
}
