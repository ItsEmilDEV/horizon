use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

pub const FILE: &str = "belumi-waypoints.toml";

pub const COLORS: [u32; 8] = [
    0xFF5555, 0x55FF55, 0x5555FF, 0xFFFF55, 0xFF55FF, 0x55FFFF, 0xFFAA00, 0xFFFFFF,
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Waypoint {
    pub scope: String,
    pub dimension: String,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub color: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Waypoints {
    #[serde(default, rename = "waypoint")]
    pub list: Vec<Waypoint>,
}

impl Waypoints {
    pub fn load(dir: &Path) -> Self {
        let path = dir.join(FILE);
        let Ok(text) = fs::read_to_string(&path) else {
            return Self::default();
        };
        toml::from_str(&text).unwrap_or_else(|_| {
            let _ = fs::rename(&path, path.with_extension("toml.bak"));
            Self::default()
        })
    }
    pub fn save(&self, dir: &Path) {
        let path = dir.join(FILE);
        if self.list.is_empty() && !path.exists() {
            return;
        }
        if let Ok(text) = toml::to_string_pretty(self) {
            let _ = fs::write(path, text);
        }
    }
    pub fn here<'a>(
        &'a self,
        scope: &'a str,
        dimension: &'a str,
    ) -> impl Iterator<Item = &'a Waypoint> {
        self.list
            .iter()
            .filter(move |w| w.scope == scope && w.dimension == dimension)
    }
    pub fn add(
        &mut self,
        scope: &str,
        dimension: &str,
        name: &str,
        at: [i32; 3],
    ) -> Result<&Waypoint, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("A waypoint needs a name.".into());
        }
        if self
            .list
            .iter()
            .any(|w| w.scope == scope && w.name.eq_ignore_ascii_case(name))
        {
            return Err(format!("There is already a waypoint called {name}."));
        }
        let color = COLORS[self.list.iter().filter(|w| w.scope == scope).count() % COLORS.len()];
        self.list.push(Waypoint {
            scope: scope.into(),
            dimension: dimension.into(),
            name: name.into(),
            x: at[0],
            y: at[1],
            z: at[2],
            color,
        });
        Ok(self.list.last().unwrap())
    }
    pub fn next_name(&self, scope: &str) -> String {
        (1..)
            .map(|n| format!("Waypoint {n}"))
            .find(|n| {
                !self
                    .list
                    .iter()
                    .any(|w| w.scope == scope && w.name.eq_ignore_ascii_case(n))
            })
            .unwrap()
    }
    pub fn remove(&mut self, scope: &str, name: &str) -> bool {
        let before = self.list.len();
        self.list
            .retain(|w| !(w.scope == scope && w.name.eq_ignore_ascii_case(name.trim())));
        self.list.len() != before
    }
    pub fn nearest<'a>(
        &'a self,
        scope: &'a str,
        dimension: &'a str,
        from: [f64; 3],
        count: usize,
    ) -> Vec<(&'a Waypoint, f64)> {
        let mut found: Vec<_> = self
            .here(scope, dimension)
            .map(|w| (w, distance(w, from)))
            .collect();
        found.sort_by(|a, b| a.1.total_cmp(&b.1));
        found.truncate(count);
        found
    }
}

const XAERO_COLORS: [u32; 16] = [
    0x000000, 0x0000AA, 0x00AA00, 0x00AAAA, 0xAA0000, 0xAA00AA, 0xFFAA00, 0xAAAAAA, 0x555555, 0x5555FF, 0x55FF55,
    0x55FFFF, 0xFF5555, 0xFF55FF, 0xFFFF55, 0xFFFFFF,
];

/// The dimension a Xaero folder name (`dim%0`, `dim%-1`, `dim%1`) stands for.
pub fn xaero_dimension(folder: &str) -> Option<&'static str> {
    match folder.strip_prefix("dim%")? {
        "0" => Some("minecraft:overworld"),
        "-1" => Some("minecraft:the_nether"),
        "1" => Some("minecraft:the_end"),
        _ => None,
    }
}

/// The waypoints of one Xaero file (`waypoint:name:initials:x:y:z:color:disabled:type:set:...`).
/// Death points (type 1) and lines it cannot read are left out.
pub fn parse_xaero(text: &str, scope: &str, dimension: &str) -> Vec<Waypoint> {
    text.lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.trim().split(':').collect();
            if parts.first() != Some(&"waypoint") || parts.len() < 7 {
                return None;
            }
            let coord = |s: &str| s.parse::<i32>().ok();
            let x = coord(parts[3])?;
            let y = if parts[4] == "~" { 64 } else { coord(parts[4])? };
            let z = coord(parts[5])?;
            if parts.get(8) == Some(&"1") {
                return None;
            }
            let name = parts[1].replace("^col^", ":");
            if name.trim().is_empty() {
                return None;
            }
            let color = parts[6].parse::<usize>().ok().and_then(|i| XAERO_COLORS.get(i)).copied().unwrap_or(0xFFFFFF);
            Some(Waypoint { scope: scope.into(), dimension: dimension.into(), name, x, y, z, color })
        })
        .collect()
}

impl Waypoints {
    /// Reads Xaero waypoint files from `path` (one file, or a folder searched up to four levels deep),
    /// takes the dimension from a `dim%N` folder on the way and `fallback` otherwise.
    /// Returns (added, skipped as already present).
    pub fn import_xaero(&mut self, path: &Path, scope: &str, fallback: &str) -> Result<(usize, usize), String> {
        let mut files = Vec::new();
        collect_files(path, fallback, 0, &mut files);
        if files.is_empty() {
            return Err(format!("No waypoint file found at {}.", path.display()));
        }
        let (mut added, mut skipped) = (0, 0);
        for (file, dimension) in files {
            let Ok(text) = fs::read_to_string(&file) else { continue };
            for wp in parse_xaero(&text, scope, &dimension) {
                if self.list.iter().any(|w| w.scope == scope && w.dimension == wp.dimension && w.name.eq_ignore_ascii_case(&wp.name)) {
                    skipped += 1;
                } else {
                    self.list.push(wp);
                    added += 1;
                }
            }
        }
        Ok((added, skipped))
    }
}

fn collect_files(path: &Path, dimension: &str, depth: usize, out: &mut Vec<(std::path::PathBuf, String)>) {
    if path.is_file() {
        out.push((path.to_path_buf(), dimension.to_owned()));
        return;
    }
    if depth > 4 {
        return;
    }
    let Ok(entries) = fs::read_dir(path) else { return };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let p = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if p.is_dir() {
            let dim = xaero_dimension(&name).unwrap_or(dimension);
            collect_files(&p, dim, depth + 1, out);
        } else if name.ends_with(".txt") && name != "config.txt" {
            out.push((p, dimension.to_owned()));
        }
    }
}

pub fn distance(w: &Waypoint, from: [f64; 3]) -> f64 {
    let dx = f64::from(w.x) + 0.5 - from[0];
    let dy = f64::from(w.y) - from[1];
    let dz = f64::from(w.z) + 0.5 - from[2];
    (dx * dx + dy * dy + dz * dz).sqrt()
}

pub fn bearing(w: &Waypoint, from: [f64; 3], yaw: f32) -> f32 {
    let dx = f64::from(w.x) + 0.5 - from[0];
    let dz = f64::from(w.z) + 0.5 - from[2];
    let target = (-dx.atan2(dz)).to_degrees() as f32;
    let mut d = (target - yaw).rem_euclid(360.0);
    if d > 180.0 {
        d -= 360.0;
    }
    d
}

pub fn arrow(relative: f32) -> &'static str {
    match ((relative + 180.0).rem_euclid(360.0) / 45.0 + 0.5) as u32 % 8 {
        0 => "v",
        1 => "<v",
        2 => "<",
        3 => "<^",
        4 => "^",
        5 => "^>",
        6 => ">",
        _ => ">v",
    }
}

pub fn distance_label(blocks: f64) -> String {
    if blocks >= 1000.0 {
        format!("{:.1} km", blocks / 1000.0)
    } else {
        format!("{blocks:.0} m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Waypoints {
        let mut w = Waypoints::default();
        w.add("w1", "overworld", "Home", [0, 64, 0]).unwrap();
        w.add("w1", "overworld", "Mine", [100, 12, 0]).unwrap();
        w.add("w1", "nether", "Portal", [5, 70, 5]).unwrap();
        w.add("w2", "overworld", "Other", [1, 1, 1]).unwrap();
        w
    }

    #[test]
    fn names_are_unique_per_scope_and_never_empty() {
        let mut w = sample();
        assert!(w.add("w1", "overworld", "home", [1, 1, 1]).is_err());
        assert!(w.add("w3", "overworld", "home", [1, 1, 1]).is_ok());
        assert!(w.add("w1", "overworld", "  ", [1, 1, 1]).is_err());
        assert_eq!(w.next_name("w1"), "Waypoint 1");
    }

    #[test]
    fn nearest_filters_by_world_and_dimension_and_sorts() {
        let w = sample();
        let near = w.nearest("w1", "overworld", [90.0, 64.0, 0.0], 5);
        assert_eq!(near.len(), 2);
        assert_eq!(near[0].0.name, "Mine");
    }

    #[test]
    fn xaero_files_import_with_dimension_colour_and_escaped_colons() {
        let text = "#comment\nsets:gui.xaero_default\nwaypoint:Base:B:10:64:-20:12:false:0:gui.xaero_default:false:0:0:false\nwaypoint:A^col^B:A:1:~:2:4:false:0:gui.xaero_default:false:0:0:false\nwaypoint:gone:G:5:5:5:0:false:1:gui.xaero_default:false:0:0:false\nwaypoint:bad:B:x:1:1:0:false:0:s\n";
        let list = parse_xaero(text, "w", "minecraft:the_nether");
        assert_eq!(list.len(), 2);
        assert_eq!((list[0].name.as_str(), list[0].x, list[0].z, list[0].color), ("Base", 10, -20, 0xFF5555));
        assert_eq!((list[1].name.as_str(), list[1].y), ("A:B", 64));
        assert_eq!(xaero_dimension("dim%-1"), Some("minecraft:the_nether"));
        let mut store = Waypoints::default();
        let dir = std::env::temp_dir().join(format!("belumi-xaero-{}", std::process::id()));
        let sub = dir.join("Multiplayer_x").join("dim%1");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("mw$default_1.txt"), text).unwrap();
        assert_eq!(store.import_xaero(&dir, "w", "minecraft:overworld").unwrap(), (2, 0));
        assert_eq!(store.import_xaero(&dir, "w", "minecraft:overworld").unwrap(), (0, 2));
        assert!(store.list.iter().all(|w| w.dimension == "minecraft:the_end"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_and_round_trip() {
        let mut w = sample();
        assert!(w.remove("w1", "HOME"));
        assert!(!w.remove("w1", "home"));
        let text = toml::to_string_pretty(&w).unwrap();
        assert_eq!(toml::from_str::<Waypoints>(&text).unwrap(), w);
    }

    #[test]
    fn bearing_is_relative_to_the_view() {
        let w = Waypoint {
            scope: String::new(),
            dimension: String::new(),
            name: String::new(),
            x: 0,
            y: 0,
            z: 10,
            color: 0,
        };
        let south = bearing(&w, [0.5, 0.0, 0.0], 0.0);
        assert!(south.abs() < 1.0);
        let behind = bearing(&w, [0.5, 0.0, 0.0], 180.0);
        assert!((behind.abs() - 180.0).abs() < 1.0);
        assert_eq!(arrow(0.0), "^");
        assert_eq!(arrow(180.0), "v");
        assert_eq!(arrow(90.0), ">");
        assert_eq!(distance_label(1500.0), "1.5 km");
        assert_eq!(distance_label(42.4), "42 m");
    }
}
