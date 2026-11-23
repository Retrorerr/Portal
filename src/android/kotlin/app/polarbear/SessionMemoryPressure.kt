package app.polarbear

internal class SessionMemoryPressure {
    private var changedSamples = 0
    private var warning = false

    /** Return true only when a sustained change should update the notification. */
    fun update(low: Boolean): Boolean {
        if (low == warning) {
            changedSamples = 0
            return false
        }
        val samplesRequired = if (low) 3 else 6
        if (++changedSamples < samplesRequired) return false
        warning = low
        changedSamples = 0
        return true
    }

    data class Swap(val totalKiB: Long, val freeKiB: Long)

    companion object {
        fun isLow(systemLow: Boolean, available: Long, total: Long, swap: Swap?): Boolean {
            // Swap can stay in use long after pressure subsides. Use it as an early
            // signal only with very little available RAM (at most 1/16 of the device).
            return systemLow || (
                total > 0 && available in 0..(total / 16) &&
                    swap != null && swap.freeKiB <= swap.totalKiB / 3
            )
        }

        fun readSwap(lines: Sequence<String>): Swap? {
            var total: Long? = null
            var free: Long? = null
            for (line in lines) {
                when (line.substringBefore(':')) {
                    "SwapTotal" -> total = amountKiB(line)
                    "SwapFree" -> free = amountKiB(line)
                }
                if (total != null && free != null) {
                    return if (total > 0 && free in 0..total) Swap(total, free) else null
                }
            }
            return null
        }

        private fun amountKiB(line: String): Long? =
            line.substringAfter(':').trimStart().takeWhile { !it.isWhitespace() }.toLongOrNull()
    }
}
