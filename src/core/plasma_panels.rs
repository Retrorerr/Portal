//! Which screen edges hold a Plasma panel that shows on an empty desktop.
//!
//! The veil waits for the desktop to be worth showing. The wallpaper arrives
//! first and the panel about a second later, so the veil also watches the
//! edges that have a panel. Reading the user's own panel layout means a
//! desktop without one, or with an auto-hiding one, never waits for it.

/// A screen edge, as Plasma's `location` numbers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

impl Edge {
    fn from_location(location: &str) -> Option<Self> {
        match location.trim() {
            "3" => Some(Self::Top),
            "4" => Some(Self::Bottom),
            "5" => Some(Self::Left),
            "6" => Some(Self::Right),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Bottom => "bottom",
            Self::Left => "left",
            Self::Right => "right",
        }
    }
}

/// Plasma's `panelVisibility` for a panel that hides until the pointer
/// reaches its edge.
const AUTO_HIDE: &str = "1";

/// The edges with a visible panel, from the text of
/// `plasma-org.kde.plasma.desktop-appletsrc` and `plasmashellrc`.
pub fn visible_panel_edges(appletsrc: &str, plasmashellrc: &str) -> Vec<Edge> {
    let mut edges = Vec::new();
    let mut current: Option<String> = None;
    let mut panel = false;
    let mut location: Option<Edge> = None;
    let mut finish = |id: Option<String>, panel: bool, location: Option<Edge>| {
        let (Some(id), true, Some(edge)) = (id, panel, location) else {
            return;
        };
        if panel_visibility(plasmashellrc, &id) != Some(AUTO_HIDE) && !edges.contains(&edge) {
            edges.push(edge);
        }
    };
    for line in appletsrc.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            finish(current.take(), panel, location);
            panel = false;
            location = None;
            current = containment_id(line);
        } else if current.is_some() {
            match line.split_once('=') {
                Some(("plugin", value)) => panel = value.trim() == "org.kde.panel",
                Some(("location", value)) => location = Edge::from_location(value),
                _ => {}
            }
        }
    }
    finish(current, panel, location);
    edges
}

/// `[Containments][7]` gives `7`; sub-groups such as applets give nothing.
fn containment_id(header: &str) -> Option<String> {
    let rest = header.strip_prefix("[Containments][")?;
    let id = rest.strip_suffix(']')?;
    (!id.is_empty() && id.chars().all(|c| c.is_ascii_digit())).then(|| id.to_owned())
}

fn panel_visibility<'a>(plasmashellrc: &'a str, id: &str) -> Option<&'a str> {
    let header = format!("[PlasmaViews][Panel {id}]");
    let mut inside = false;
    for line in plasmashellrc.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside = line == header;
        } else if inside {
            if let Some(("panelVisibility", value)) = line.split_once('=') {
                return Some(value.trim());
            }
        }
    }
    None
}
