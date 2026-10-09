// Portal (Debian 13, Plasma 6.3) version of ../../files/panel-layout.js — see apply-portal.sh
//   qdbus6 org.kde.plasmashell /PlasmaShell org.kde.PlasmaShell.evaluateScript "$(cat panel-layout.js)"
// @BAR@, @FONT@, @HOME@ are substituted by apply.sh.
const BAR = @BAR@;
const DOCK = @DOCK@;
const FONT = "@FONT@";
const C = { bg: "#1c1b1b", fg: "#ddd6d0", green: "#a9b665", yellow: "#d8a657", red: "#ea6962" };

panels().forEach(function (p) { p.remove(); });

const p = new Panel("org.kde.panel");
p.location = "top";
p.height = BAR;
p.floating = false;
p.hiding = "none";
p.alignment = "center";

// stock Kickoff: a Meta tap opens it, Meta or a click elsewhere closes it.
// Alt+F1 is only a temporary handle: apply.sh finds the widget by it, then rebinds it to Meta alone
const kick = p.addWidget("org.kde.plasma.kickoff");
kick.globalShortcut = "Alt+F1";
kick.currentConfigGroup = ["General"];
kick.writeConfig("icon", "start-here-kde-symbolic");
const desks = p.addWidget("org.rice.desktopnumbers");
p.addWidget("org.kde.plasma.panelspacer");

const clock = p.addWidget("org.kde.plasma.digitalclock");
clock.currentConfigGroup = ["Appearance"];
clock.writeConfig("showDate", true);
clock.writeConfig("dateFormat", "custom");
clock.writeConfig("customDateFormat", "d MMM");
clock.writeConfig("dateDisplayFormat", "BesideTime");
clock.writeConfig("autoFontAndSize", false);
clock.writeConfig("fontFamily", "Inter");
clock.writeConfig("fontSize", 10);
clock.writeConfig("fontWeight", 500);
clock.writeConfig("showSeconds", "Never");

p.addWidget("org.kde.plasma.panelspacer");

const music = p.addWidget("plasmusic-toolbar");
music.currentConfigGroup = ["General"];
music.writeConfig("panelIcon", "media-optical-audio");
music.writeConfig("skipBackwardControlInPanel", false);
music.writeConfig("playPauseControlInPanel", false);
music.writeConfig("skipForwardControlInPanel", false);
music.writeConfig("maxSongWidthInPanel", 180);
music.writeConfig("useCustomFont", true);
music.writeConfig("customFont", FONT);
music.writeConfig("showWhenNoMedia", false);   // hides itself when nothing plays
music.writeConfig("artistsPosition", 0);       // title only, like the reference
music.writeConfig("textScrollingBehaviour", 0);
music.writeConfig("textScrollingEnabled", false);   // long titles are cut off instead of scrolling
music.writeConfig("useAlbumCoverAsPanelIcon", true);
// popup: rounded album art, no loop / shuffle / volume buttons
music.writeConfig("fullAlbumCoverRadius", 26);
music.writeConfig("fullViewLoopVisible", false);
music.writeConfig("fullViewShuffleVisible", false);
music.writeConfig("fullViewVolumeControlVisible", false);

const vol = p.addWidget("org.kde.plasma.volume");
const notif = p.addWidget("org.kde.plasma.notifications");

const tray = p.addWidget("org.kde.plasma.systemtray");
tray.currentConfigGroup = ["General"];   // Plasma 6.7: the tray is itself a containment; config lives here
// everything a laptop needs stays reachable behind the arrow; only these three are always visible
// Portal: no battery / NetworkManager / bluetooth inside PRoot (Android owns those); keep the useful ones
const trayItems = ["org.kde.plasma.clipboard", "org.kde.plasma.keyboardlayout", "org.kde.plasma.devicenotifier",
  "org.kde.plasma.manage-inputmethod", "org.kde.kscreen"];
// app status icons shown when they're active (the rest of the apps below stay behind the arrow)
const trayApps = ["steam"];
tray.writeConfig("extraItems", trayItems.concat(trayApps));
// knownItems must list every tray-capable widget: anything missing counts as "new" and the tray
// switches it on (that brought back a second volume icon, plus notifications/weather/media)
tray.writeConfig("knownItems", trayItems.concat(["org.kde.plasma.volume", "org.kde.plasma.notifications",
  "org.kde.plasma.mediacontroller", "org.kde.plasma.weather", "org.kde.plasma.printmanager", "org.kde.plasma.vault"]));
tray.writeConfig("shownItems", []);
// these app status icons (yours, by SNI id) go behind the arrow; they still pop out when they need attention
tray.writeConfig("hiddenItems", trayItems);
tray.writeConfig("showAllItems", false);
tray.writeConfig("scaleIconsToFit", false);
tray.writeConfig("iconSpacing", 1);

const power = p.addWidget("org.kde.plasma.icon");
power.currentConfigGroup = ["General"];
power.writeConfig("url", "file://@HOME@/.local/share/applications/rice-powermenu.desktop");
power.writeConfig("icon", "system-shutdown-symbolic");

// Panel Colorizer: flat #1c1b1b, no native background/shadow, cloud-white text, coloured volume/bell/power
const col = p.addWidget("luisbocanegra.panel.colorizer");
col.currentConfigGroup = ["General"];
function solid(hex) { return { enabled: true, sourceType: 0, custom: hex, alpha: 1 }; }
const global = {
  panel: { normal: { enabled: true, backgroundColor: solid(C.bg) } },
  nativePanel: { background: { enabled: false, opacity: 0, shadow: false } },
  widgets: { normal: { enabled: true, foregroundColor: solid(C.fg) } },
  trayWidgets: { normal: { enabled: true, foregroundColor: solid(C.fg) } }
};
function fgOverride(hex) {
  return { disabledFallback: true, normal: { enabled: true, foregroundColor: solid(hex) } };
}
const overrides = {
  overrides: { "rice-green": fgOverride(C.green), "rice-yellow": fgOverride(C.yellow), "rice-red": fgOverride(C.red) },
  associations: [
    { id: vol.id, name: "org.kde.plasma.volume", presets: ["rice-green"] },
    { id: notif.id, name: "org.kde.plasma.notifications", presets: ["rice-yellow"] },
    { id: power.id, name: "org.kde.plasma.icon", presets: ["rice-red"] }
  ]
};
col.writeConfig("isEnabled", true);
col.writeConfig("hideWidget", true);
col.writeConfig("globalSettings", JSON.stringify(global));

// Minimal dock: left edge, vertically centred, floating, icons only, tucks away when a window overlaps it
const dock = new Panel("org.kde.panel");
dock.location = "left";
dock.height = DOCK;
dock.floating = true;
dock.lengthMode = "fit";
dock.alignment = "center";
dock.hiding = "dodgewindows";
const tasks = dock.addWidget("org.kde.plasma.icontasks");
tasks.currentConfigGroup = ["General"];
tasks.writeConfig("launchers", ["applications:org.kde.dolphin.desktop", "preferred://browser",
  "applications:org.kde.konsole.desktop", "applications:systemsettings.desktop"]);
tasks.writeConfig("showOnlyCurrentDesktop", true);
tasks.writeConfig("groupingStrategy", 1);          // group windows of one app under one icon
tasks.writeConfig("groupedTaskVisualization", 1);
tasks.writeConfig("indicateAudioStreams", true);
tasks.writeConfig("iconSpacing", 1);
tasks.writeConfig("maxStripes", 1);
tasks.writeConfig("middleClickAction", "NewInstance");

print("<<OV" + JSON.stringify(overrides) + "OV>>");
print("rice-panel ids: desks=" + desks.id + " clock=" + clock.id + " vol=" + vol.id + " notif=" + notif.id +
      " tray=" + tray.id + " power=" + power.id + " colorizer=" + col.id);
