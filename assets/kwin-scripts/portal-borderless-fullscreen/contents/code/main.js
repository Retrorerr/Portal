// Managed by Portal: borderless fullscreen games become real fullscreen.
//
// Games in "borderless" or "windowed fullscreen" mode (Wine/Proton, SDL,
// Unity) open an undecorated normal window the size of the screen and never
// ask for fullscreen; many make it a pixel short so the driver will not take
// exclusive mode. KWin then keeps the panel above it. Windows and Mutter
// treat such a window as fullscreen, so do the same: X11 windows only
// (Wayland clients ask for fullscreen themselves), normal windows without
// a border, covering their output to within a few pixels. Wine drops the
// state again when the game goes back to a smaller window.

const SLACK = 2;

function coversOutput(window) {
    const output = window.output;
    if (!output) {
        return false;
    }
    const frame = window.frameGeometry;
    const screen = output.geometry;
    return Math.abs(frame.x - screen.x) <= SLACK
        && Math.abs(frame.y - screen.y) <= SLACK
        && Math.abs(frame.width - screen.width) <= SLACK
        && Math.abs(frame.height - screen.height) <= SLACK;
}

// KWin 6.3 exposes no window id to scripts; the wrapper's QObject class
// name ("KWin::X11Window(0x...)") tells X11 windows apart.
function isX11(window) {
    return String(window).indexOf("X11Window") !== -1;
}

function check(window) {
    if (window.deleted || window.fullScreen || !window.normalWindow
        || !window.noBorder || !window.fullScreenable || !isX11(window)) {
        return;
    }
    if (coversOutput(window)) {
        window.fullScreen = true;
    }
}

function watch(window) {
    check(window);
    window.frameGeometryChanged.connect(() => check(window));
}

workspace.windowList().forEach(watch);
workspace.windowAdded.connect(watch);
