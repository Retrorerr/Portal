package app.polarbear

// Debug builds only (PortalActivity checks FLAG_DEBUGGABLE): plays a scripted
// update on the real Return-to-Plasma screen so every state of the update
// panel can be reviewed without a pending release or a Debian upgrade.
// Nothing native runs; Update drives the script instead of the coordinator,
// and native state is ignored until the preview is turned off.
//
//   adb shell am start -n app.polarbear/.PortalActivity --activity-single-top \
//       --es portal_preview_updates full|app|debian|fail|off

import android.os.Handler
import android.os.Looper
import android.util.Log

object UpdatePreview {
    const val EXTRA = "portal_preview_updates"
    private const val TAG = "UpdatePreview"
    private const val TICK_MS = 450L

    private val main = Handler(Looper.getMainLooper())

    private val packages = listOf(
        "libssl3t64" to true,
        "openssl" to true,
        "libc6" to true,
        "firefox" to false,
        "tzdata" to false,
        "libcurl4t64" to false,
        "libsystemd0" to false,
        "python3.13" to false,
        "libglib2.0-0t64" to false,
        "claude-desktop" to false,
        "libpng16-16t64" to false,
        "vim-tiny" to false,
        "libxml2" to false,
        "curl" to false,
    )
    private const val APP = "app\tPortal 2.5.0\tSmoother on-screen controller and faster Proton starts"
    private const val DRIVER = "driver\tGraphics driver\tMesa 26.3.0-20261010 from lfdevs"
    private const val CHATGPT = "chatgpt\tChatGPT\tA newer build from OpenAI"

    private data class Scenario(val packages: List<Pair<String, Boolean>>, val components: List<String>)

    private class Step(val kind: String, val title: String) {
        var state = ComposeOverlay.STEP_PENDING
        var note = ""
    }

    private var scenario = Scenario(emptyList(), emptyList())
    private var steps = emptyList<Step>()
    private var failing = false

    fun play(name: String) {
        main.removeCallbacksAndMessages(null)
        failing = name == "fail"
        steps = emptyList()
        scenario = when (name) {
            "full", "fail" -> Scenario(packages, listOf(APP, DRIVER, CHATGPT))
            "app" -> Scenario(emptyList(), listOf(APP))
            "debian" -> Scenario(packages, emptyList())
            else -> {
                ComposeOverlay.previewBegin = null
                publish(ComposeOverlay.SYSTEM_UPDATE_NONE, 0, "", Scenario(emptyList(), emptyList()))
                Log.i(TAG, "update preview off")
                return
            }
        }
        ComposeOverlay.previewBegin = ::begin
        publish(ComposeOverlay.SYSTEM_UPDATE_AVAILABLE, 0, "", scenario)
        Log.i(TAG, "update preview: $name")
    }

    private fun has(kind: String) = scenario.components.any { it.startsWith("$kind\t") }

    private fun publish(status: Int, progress: Int, message: String, shown: Scenario) {
        ComposeOverlay.applySystemUpdateState(
            status,
            progress,
            message,
            shown.packages.size,
            shown.packages.count { it.second },
            false,
            shown.packages.joinToString("\n") { "${it.first}\t${if (it.second) 1 else 0}" },
            shown.components.joinToString("\n"),
            steps.joinToString("\n") { "${it.kind}\t${it.state}\t${it.title}\t${it.note}" },
        )
    }

    private fun begin(includeApp: Boolean): Boolean {
        val withApp = has("app") && includeApp
        steps = buildList {
            if (has("driver")) add(Step("driver", "Graphics driver"))
            if (scenario.packages.isNotEmpty()) add(Step("debian", "Debian packages"))
            if (has("chatgpt")) add(Step("chatgpt", "ChatGPT"))
            if (withApp) add(Step("app", "Portal 2.5.0"))
        }
        // (progress, message, step changes to apply first)
        val script = mutableListOf<Triple<Int, String, () -> Unit>>()
        fun at(progress: Int, message: String, change: () -> Unit = {}) {
            script += Triple(progress, message, change)
        }
        fun step(kind: String) = steps.first { it.kind == kind }
        at(1, "Closing Plasma…")
        if (has("driver")) {
            at(4, "Downloading the graphics driver…") { step("driver").state = ComposeOverlay.STEP_ACTIVE }
            for (percent in 6..14 step 2) at(percent, "Downloading the graphics driver…")
            at(16, "Unpacking the graphics driver…")
            at(18, "Installing the graphics driver…")
        }
        if (scenario.packages.isNotEmpty()) {
            at(20, "Refreshing package lists…") {
                if (has("driver")) step("driver").state = ComposeOverlay.STEP_DONE
                step("debian").state = ComposeOverlay.STEP_ACTIVE
            }
            for (percent in 24..44 step 4) at(percent, "Downloading updates…")
            scenario.packages.forEachIndexed { index, (name, _) -> at(46 + index * 2, "Unpacking $name…") }
            if (failing) {
                at(74, "Checking Portal's desktop…") {
                    step("debian").apply {
                        state = ComposeOverlay.STEP_FAILED
                        note = "Portal refused this upgrade because it would remove libkwin6; nothing was installed"
                    }
                }
            } else {
                at(76, "Setting up openssl…")
                at(78, "Checking Portal's desktop…")
            }
        }
        if (has("chatgpt")) {
            at(82, "Downloading ChatGPT…") {
                if (!failing) steps.firstOrNull { it.kind == "debian" }?.state = ComposeOverlay.STEP_DONE
                step("chatgpt").state = ComposeOverlay.STEP_ACTIVE
            }
            at(86, "Setting up chatgpt…")
        }
        if (withApp) {
            at(88, "Downloading Portal 2.5.0…") {
                if (has("chatgpt")) step("chatgpt").state = ComposeOverlay.STEP_DONE
                step("app").state = ComposeOverlay.STEP_ACTIVE
            }
            for (percent in 90..94 step 2) at(percent, "Downloading Portal 2.5.0…")
            at(96, "Installing Portal 2.5.0…")
        }
        script.forEachIndexed { index, (progress, message, change) ->
            main.postDelayed({
                change()
                publish(ComposeOverlay.SYSTEM_UPDATE_RUNNING, progress, message, scenario)
            }, index * TICK_MS)
        }
        main.postDelayed({
            steps.filter { it.state == ComposeOverlay.STEP_ACTIVE }.forEach {
                it.state = ComposeOverlay.STEP_DONE
            }
            if (failing) {
                publish(
                    ComposeOverlay.SYSTEM_UPDATE_FAILED,
                    96,
                    "Debian: Portal refused this upgrade because it would remove libkwin6; nothing was installed",
                    scenario,
                )
            } else {
                publish(ComposeOverlay.SYSTEM_UPDATE_COMPLETE, 100, "Restarting Plasma…", Scenario(emptyList(), emptyList()))
            }
        }, script.size * TICK_MS + 600L)
        return true
    }
}
