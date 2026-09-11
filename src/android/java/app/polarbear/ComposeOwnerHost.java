package app.polarbear;

import android.view.View;
import androidx.lifecycle.LifecycleOwner;
import androidx.lifecycle.ViewModelStoreOwner;
import androidx.lifecycle.ViewTreeLifecycleOwner;
import androidx.lifecycle.ViewTreeViewModelStoreOwner;
import androidx.savedstate.SavedStateRegistryOwner;
import androidx.savedstate.ViewTreeSavedStateRegistryOwner;

/**
 * SPIKE-ONLY (branch compose-setup-spike): Java bridge for the Compose overlay owner wiring.
 *
 * <p>The {@code ViewTree*} owner setters ship as metadata-less Java facades inside the
 * KMP-published lifecycle/savedstate artifacts: {@code javac} resolves them, but
 * {@code kotlinc} reports them as unresolved references. Calling them from Java keeps
 * the Kotlin overlay free of reflection while supplying the LifecycleOwner /
 * SavedStateRegistryOwner / ViewModelStoreOwner that ComposeView requires inside
 * Portal's NativeActivity.
 */
public final class ComposeOwnerHost {
    private ComposeOwnerHost() {}

    public static void attach(
            View view,
            LifecycleOwner lifecycleOwner,
            ViewModelStoreOwner viewModelStoreOwner,
            SavedStateRegistryOwner savedStateRegistryOwner) {
        ViewTreeLifecycleOwner.set(view, lifecycleOwner);
        ViewTreeViewModelStoreOwner.set(view, viewModelStoreOwner);
        ViewTreeSavedStateRegistryOwner.set(view, savedStateRegistryOwner);
    }
}
