package app.polarbear.setup

// Plain-language reading of the native install snapshot. The native side
// reports overall progress (download 5..45, verify 45..52, unpack 52..70,
// configure 70..99, finish 99..100) and a technical message; the setup
// screen and the progress notification both show this instead.

/** The four steps the user sees, in order. */
val INSTALL_STEP_TITLES = listOf("Download", "Unpack", "Set up", "Finish")

data class InstallStep(
    /** Index into [INSTALL_STEP_TITLES]. */
    val index: Int,
    /** What Portal is doing, stable while it keeps doing it. */
    val label: String,
    /** The live count that goes with [label], if any ("58 of 964 MB"). */
    val figure: String? = null,
    /** Portal is waiting on the network rather than working. */
    val waiting: Boolean = false,
) {
    /** One line for places that cannot animate the figure on its own. */
    val detail: String get() = figure?.let { "$label · $it" } ?: "$label…"
}

private val DOWNLOADED = Regex("""Downloading Debian runtime: (\d+) / (\d+) MiB""")
private val EXTRACTED = Regex("""Extracting Debian runtime: (\d+) entries""")
private val STAGE = Regex("""Configuring Portal \(([a-z0-9-]+)\)""")

fun installStep(progress: Int, message: String): InstallStep {
    val index = when {
        progress < 45 -> 0
        progress < 70 -> 1
        progress < 99 -> 2
        else -> 3
    }
    if (message.startsWith("Waiting for an internet connection")) {
        return InstallStep(index, "Waiting for an internet connection", waiting = true)
    }
    if (message.startsWith("Something went wrong")) {
        return InstallStep(index, "Something went wrong, trying again")
    }
    DOWNLOADED.find(message)?.let { match ->
        val (done, total) = match.destructured
        return InstallStep(index, "Downloading Debian", "${mb(done)} of ${mb(total)} MB")
    }
    EXTRACTED.find(message)?.let { match ->
        val count = match.groupValues[1].toIntOrNull() ?: 0
        return InstallStep(index, "Unpacking Debian", "${"%,d".format(count)} files")
    }
    val label = when (index) {
        0 -> "Downloading Debian"
        1 -> if (progress < 52) "Checking the download" else "Unpacking Debian"
        2 -> when (STAGE.find(message)?.groupValues?.get(1)) {
            "mesa-kgsl-layer" -> "Setting up graphics"
            "desktop-login" -> "Preparing your account"
            "optional-apps" -> "Installing your apps"
            else -> if (message.contains("graphics", ignoreCase = true)) {
                "Setting up graphics"
            } else {
                "Setting up KDE Plasma"
            }
        }
        else -> if (message.contains("appearance", ignoreCase = true)) {
            "Applying your appearance and size"
        } else {
            "Starting Plasma"
        }
    }
    return InstallStep(index, label)
}

/** Where setup stopped, said from the user's side of the step track. */
fun pausedLabel(index: Int): String = when (index) {
    0 -> "Stopped while downloading"
    1 -> "Stopped while unpacking"
    2 -> "Stopped while setting up"
    else -> "Stopped while finishing"
}

/** "Installed in 4 min 12 s", for an install this screen watched from start to end. */
fun installedIn(millis: Long): String {
    val seconds = ((millis + 500) / 1000).coerceAtLeast(1)
    return if (seconds < 60) {
        "Installed in $seconds s"
    } else {
        val rest = seconds % 60
        "Installed in ${seconds / 60} min" + if (rest > 0) " $rest s" else ""
    }
}

/** MiB as the rounded MB a user expects to read. */
private fun mb(mib: String): String = ((mib.toLongOrNull() ?: 0L) * 1_048_576L / 1_000_000L).toString()
