package app.polarbear

import android.net.LocalServerSocket
import android.net.LocalSocket
import android.os.Process
import android.util.Log
import java.io.IOException
import java.util.concurrent.atomic.AtomicInteger
import kotlin.concurrent.thread

/** A guest connection holds the display awake until it closes, including on process exit. */
internal class ScreenInhibition(private val onChanged: () -> Unit) {
    private val connections = AtomicInteger()
    val active: Boolean get() = connections.get() > 0

    fun start() {
        // The desktop launcher passes the same per-process name to the guest.
        val server = try {
            LocalServerSocket("portal-screen-inhibit.${Process.myPid()}")
        } catch (error: IOException) {
            Log.w("ScreenInhibition", "Could not listen for desktop display requests", error)
            return
        }
        // Portal hosts one Activity per app process. Its exit closes the listener.
        thread(name = "screen-inhibit-listener", isDaemon = true) {
            try {
                server.use {
                    while (true) accept(server.accept())
                }
            } catch (error: IOException) {
                Log.w("ScreenInhibition", "Desktop display request listener stopped", error)
            }
        }
    }

    private fun accept(socket: LocalSocket) {
        val fromGuest = try {
            socket.peerCredentials.uid == Process.myUid()
        } catch (_: IOException) {
            false
        }
        if (!fromGuest) {
            socket.close()
            return
        }
        thread(name = "screen-inhibit-connection", isDaemon = true) {
            if (connections.incrementAndGet() == 1) onChanged()
            try {
                // Clients never send data. A read waits for the guest to release the request.
                socket.use { it.inputStream.read() }
            } catch (_: IOException) {
                // A disconnected guest releases its request just like an explicit close.
            } finally {
                if (connections.decrementAndGet() == 0) onChanged()
            }
        }
    }
}
