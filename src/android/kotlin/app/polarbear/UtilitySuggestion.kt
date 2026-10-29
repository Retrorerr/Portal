package app.polarbear

/** A suggestion changes the tab's symbol, never the menu's tap behaviour. */
internal enum class UtilitySuggestion {
    Keyboard, Gaming, Files, Veil;

    companion object {
        fun forContext(textFocused: Boolean, gaming: Boolean, transferring: Boolean) = when {
            textFocused -> Keyboard
            gaming -> Gaming
            transferring -> Files
            else -> Veil
        }
    }
}
