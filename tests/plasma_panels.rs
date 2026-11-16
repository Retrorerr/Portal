//! The veil waits for the panels a desktop shows, and only those.

use localdesktop::core::plasma_panels::{visible_panel_edges, Edge};

const DEFAULT_LAYOUT: &str = "\
[ActionPlugins][0]
RightButton;NoModifier=org.kde.contextmenu

[Containments][1]
activityId=2b1c
formfactor=0
location=0
plugin=org.kde.plasma.folder

[Containments][2]
formfactor=2
lastScreen=0
location=4
plugin=org.kde.panel

[Containments][2][Applets][3]
plugin=org.kde.plasma.kickoff
location=3
";

#[test]
fn the_stock_desktop_has_one_panel_at_the_bottom() {
    assert_eq!(visible_panel_edges(DEFAULT_LAYOUT, ""), [Edge::Bottom]);
}

#[test]
fn an_auto_hiding_panel_is_not_waited_for() {
    let shell = "[PlasmaViews][Panel 2]\nfloating=1\npanelVisibility=1\n";
    assert!(visible_panel_edges(DEFAULT_LAYOUT, shell).is_empty());
    let other = "[PlasmaViews][Panel 9]\npanelVisibility=1\n";
    assert_eq!(visible_panel_edges(DEFAULT_LAYOUT, other), [Edge::Bottom]);
}

#[test]
fn a_desktop_without_panels_waits_for_none() {
    let layout = "[Containments][1]\nlocation=0\nplugin=org.kde.plasma.folder\n";
    assert!(visible_panel_edges(layout, "").is_empty());
    assert!(visible_panel_edges("", "").is_empty());
}

#[test]
fn every_panel_edge_is_listed_once() {
    let layout = "\
[Containments][2]
location=3
plugin=org.kde.panel

[Containments][5]
plugin=org.kde.panel
location=5

[Containments][8]
location=3
plugin=org.kde.panel
";
    assert_eq!(visible_panel_edges(layout, ""), [Edge::Top, Edge::Left]);
    assert_eq!(Edge::Left.name(), "left");
}
