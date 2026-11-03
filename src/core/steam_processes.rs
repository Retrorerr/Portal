//! Steam and Steam-game detection from guest process command lines.
//!
//! Steam starts every game as `reaper SteamLaunch AppId=<id> -- ...`, which
//! lives exactly as long as the game; the client itself is `steam` (with its
//! `steamwebhelper` children).

/// What one pass over the guest's processes found.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SteamScan {
    pub steam: bool,
    pub game: bool,
    pub app_id: Option<String>,
    pub name: Option<String>,
}

impl SteamScan {
    /// Account for one process's raw `/proc/<pid>/cmdline` (NUL-separated).
    pub fn add(&mut self, cmdline: &[u8]) {
        if cmdline.is_empty() {
            return;
        }
        let arg0 = cmdline.split(|&byte| byte == 0).next().unwrap_or_default();
        let base = arg0.rsplit(|&byte| byte == b'/').next().unwrap_or_default();
        match base {
            b"steam" | b"steamwebhelper" => self.steam = true,
            b"reaper" => {
                let mut args = cmdline.split(|&byte| byte == 0);
                if !args.clone().any(|arg| arg == b"SteamLaunch") {
                    return;
                }
                self.game = true;
                self.steam = true;
                if let Some(id) = args.clone().find_map(|arg| arg.strip_prefix(b"AppId=")) {
                    if !id.is_empty() && id.iter().all(u8::is_ascii_digit) && id != b"0" {
                        self.app_id = Some(String::from_utf8_lossy(id).into_owned());
                    }
                }
                // The game's folder under steamapps/common is its readable name.
                if let Some(name) = args.find_map(game_folder) {
                    self.name = Some(name);
                }
            }
            _ => {}
        }
    }

    /// The scan as `steam\tgame\tappId\tname` (`1`/`0` flags, empty when
    /// unknown); app and name are reported only while a game runs.
    pub fn encode(&self) -> String {
        let (app, name) = if self.game {
            (
                self.app_id.as_deref().unwrap_or(""),
                self.name.as_deref().unwrap_or(""),
            )
        } else {
            ("", "")
        };
        format!(
            "{}\t{}\t{}\t{}",
            u8::from(self.steam),
            u8::from(self.game),
            app,
            name.replace(['\t', '\n'], " ")
        )
    }
}

fn game_folder(arg: &[u8]) -> Option<String> {
    const MARKER: &[u8] = b"/steamapps/common/";
    let start = arg
        .windows(MARKER.len())
        .position(|window| window == MARKER)?
        + MARKER.len();
    let rest = &arg[start..];
    let folder = rest.split(|&byte| byte == b'/').next().unwrap_or_default();
    (!folder.is_empty()).then(|| String::from_utf8_lossy(folder).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(cmdlines: &[&[u8]]) -> SteamScan {
        let mut scan = SteamScan::default();
        for cmdline in cmdlines {
            scan.add(cmdline);
        }
        scan
    }

    #[test]
    fn client_alone_is_not_a_game() {
        let found = scan(&[
            b"/home/u/.local/share/Steam/ubuntu12_64/steam\0-silent\0",
            b"bash\0",
        ]);
        assert!(found.steam && !found.game);
        assert_eq!(found.encode(), "1\t0\t\t");
    }

    #[test]
    fn reaper_launch_reports_app_and_folder_name() {
        let found = scan(&[
            b"steamwebhelper\0",
            b"/x/reaper\0SteamLaunch\0AppId=2225070\0--\0/home/u/.local/share/Steam/steamapps/common/Trackmania/Trackmania.exe\0",
        ]);
        assert_eq!(found.app_id.as_deref(), Some("2225070"));
        assert_eq!(found.name.as_deref(), Some("Trackmania"));
        assert_eq!(found.encode(), "1\t1\t2225070\tTrackmania");
    }

    #[test]
    fn reaper_without_steam_launch_and_bad_ids_are_ignored() {
        assert_eq!(scan(&[b"reaper\0--\0game\0"]), SteamScan::default());
        let found = scan(&[b"reaper\0SteamLaunch\0AppId=0\0"]);
        assert!(found.game && found.app_id.is_none());
        let found = scan(&[b"reaper\0SteamLaunch\0AppId=12a\0"]);
        assert!(found.app_id.is_none());
    }

    #[test]
    fn similar_names_do_not_match() {
        assert_eq!(
            scan(&[b"/usr/bin/steamcmd\0", b"notreaper\0SteamLaunch\0", b""]),
            SteamScan::default()
        );
    }
}
