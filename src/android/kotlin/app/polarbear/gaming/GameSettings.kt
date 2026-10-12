package app.polarbear.gaming

import android.content.Context
import android.util.Log
import java.io.File

/**
 * Game settings from the quick panel, handed to Portal's Steam
 * compatibility tools as shell-sourceable `KEY=value` files.
 *
 * Host: `files/game-settings/game.conf` (every game) and
 * `files/game-settings/app-<appid>.conf` (one game; replaces game.conf for
 * it). PRoot binds the directory to `/usr/local/lib/portal/settings` in the
 * guest, and portal-proton.sh / portal-box64.sh read the file at launch.
 * A variable already set by the game's Steam launch options wins.
 *
 * The files are the only store: the panel reads them back, so what the
 * user sees is exactly what the next launch gets.
 */
data class GameConfig(
    /** Video memory DXVK reports, MB. */
    val vramMb: Int = DEFAULT_VRAM_MB,
    /** Frame-rate cap for DXVK/VKD3D games; 0 is uncapped. */
    val fpsLimit: Int = 0,
    /** PORTAL_PROTON: "" picks the newest installed build. */
    val proton: String = "",
    /** PORTAL_WOW64: "" runs 32-bit code through FEX, "box64" through Box64. */
    val wow64: String = "",
    /** FEX_DISKCACHE: keep FEX's translated code between runs. */
    val fexCache: Boolean = true,
    /** PORTAL_HUD: DXVK's frame-rate counter. */
    val hud: Boolean = false,
) {
    fun render(): String = buildString {
        append("# Written by Portal's game panel. Steam launch options override these.\n")
        if (vramMb != DEFAULT_VRAM_MB) append("PORTAL_VRAM_MB=").append(vramMb).append('\n')
        if (fpsLimit > 0) append("PORTAL_FPS_LIMIT=").append(fpsLimit).append('\n')
        if (proton.isNotEmpty()) append("PORTAL_PROTON=").append(proton).append('\n')
        if (wow64.isNotEmpty()) append("PORTAL_WOW64=").append(wow64).append('\n')
        if (!fexCache) append("FEX_DISKCACHE=0\n")
        if (hud) append("PORTAL_HUD=1\n")
    }

    companion object {
        const val DEFAULT_VRAM_MB = 4096

        fun parse(text: String): GameConfig {
            var config = GameConfig()
            for (line in text.lineSequence()) {
                val key = line.substringBefore('=', "").trim()
                val value = line.substringAfter('=', "").trim()
                config = when (key) {
                    "PORTAL_VRAM_MB" -> config.copy(vramMb = value.toIntOrNull()?.coerceIn(512, 65536) ?: DEFAULT_VRAM_MB)
                    "PORTAL_FPS_LIMIT" -> config.copy(fpsLimit = value.toIntOrNull()?.coerceAtLeast(0) ?: 0)
                    "PORTAL_PROTON" -> config.copy(proton = value)
                    "PORTAL_WOW64" -> config.copy(wow64 = if (value == "box64") "box64" else "")
                    "FEX_DISKCACHE" -> config.copy(fexCache = value != "0")
                    "PORTAL_HUD" -> config.copy(hud = value == "1")
                    else -> config
                }
            }
            return config
        }
    }
}

object GameSettings {
    private const val TAG = "PortalGameSettings"
    const val DIR = "game-settings"
    const val GUEST_DIR = "/usr/local/lib/portal/settings"
    private const val GLOBAL = "game.conf"

    private fun dir(context: Context) = File(context.filesDir, DIR)
    private fun file(context: Context, appId: String?) =
        File(dir(context), if (appId == null) GLOBAL else "app-$appId.conf")

    fun hasAppConfig(context: Context, appId: String?): Boolean =
        appId != null && file(context, appId).isFile

    /** The settings [appId] would launch with: its own file, else the shared one. */
    fun load(context: Context, appId: String?): GameConfig {
        val source = if (hasAppConfig(context, appId)) file(context, appId) else file(context, null)
        return try {
            if (source.isFile) GameConfig.parse(source.readText()) else GameConfig()
        } catch (e: Exception) {
            Log.w(TAG, "could not read $source", e)
            GameConfig()
        }
    }

    /** Save where [load] reads: the game's own file if it has one. */
    fun save(context: Context, appId: String?, config: GameConfig) {
        write(if (hasAppConfig(context, appId)) file(context, appId) else file(context, null), config)
    }

    /** Give [appId] its own copy of the shared settings, or drop it. */
    fun setAppConfig(context: Context, appId: String, own: Boolean) {
        val target = file(context, appId)
        if (own) {
            write(target, load(context, null))
        } else if (!target.delete() && target.exists()) {
            Log.w(TAG, "could not remove $target")
        }
    }

    private fun write(target: File, config: GameConfig) {
        try {
            target.parentFile?.mkdirs()
            val temporary = File(target.parentFile, target.name + ".tmp")
            temporary.writeText(config.render())
            if (!temporary.renameTo(target)) {
                target.writeText(config.render())
                temporary.delete()
            }
        } catch (e: Exception) {
            Log.w(TAG, "could not write $target", e)
        }
    }
}
