package app.polarbear.setup

// Android's Back steps back through setup the way the card's Back button
// does. Where the card has no Back, it does what it always did.

import android.content.Context
import android.content.ContextWrapper
import androidx.activity.ComponentActivity
import androidx.activity.OnBackPressedCallback
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.platform.LocalContext

@Composable
internal fun SetupBackHandler(enabled: Boolean, onBack: () -> Unit) {
    val back by rememberUpdatedState(onBack)
    val callback = remember {
        object : OnBackPressedCallback(enabled) {
            override fun handleOnBackPressed() = back()
        }
    }
    SideEffect { callback.isEnabled = enabled }
    val activity = LocalContext.current.findComponentActivity()
    DisposableEffect(activity) {
        activity?.onBackPressedDispatcher?.addCallback(activity, callback)
        onDispose { callback.remove() }
    }
}

private tailrec fun Context.findComponentActivity(): ComponentActivity? = when (this) {
    is ComponentActivity -> this
    is ContextWrapper -> baseContext.findComponentActivity()
    else -> null
}
