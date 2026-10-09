// second pass, after the widgets have loaded: Colorizer per-widget colour overrides.
// apply.sh prepends Colorizer's own code/globals.js, so baseOverrideConfig is in scope:
// each override is stored complete (Colorizer only fills top-level gaps, and a partial
// override leaves backgrounds undefined — that showed up as white squares behind the icons).
const OVERRIDES = @OVERRIDES@;
function merged(base, over) {
  const out = JSON.parse(JSON.stringify(base));
  for (const k in over) {
    out[k] = (over[k] && typeof over[k] === "object" && !Array.isArray(over[k]) && out[k] && typeof out[k] === "object")
      ? merged(out[k], over[k]) : over[k];
  }
  return out;
}
for (const name in OVERRIDES.overrides) {
  OVERRIDES.overrides[name] = merged(baseOverrideConfig, OVERRIDES.overrides[name]);
}
panels().forEach(function (p) {
  p.widgets("luisbocanegra.panel.colorizer").forEach(function (w) {
    w.currentConfigGroup = ["General"];
    w.writeConfig("configurationOverrides", JSON.stringify(OVERRIDES));
    w.reloadConfig();
    print("rice-post: overrides written to " + w.id);
  });
});
