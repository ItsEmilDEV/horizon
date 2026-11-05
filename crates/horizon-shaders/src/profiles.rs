//! A pack's quality profiles (`profile.NAME=...` in `shaders.properties`): the option values a
//! profile sets, resolved through the profiles it builds on.

/// The `(option, value)` pairs profile `name` sets. A switch is `true` or `false`; `!NAME` turns a
/// switch off, `NAME=value` sets a value, `profile.OTHER` brings in another profile first.
pub fn profile_options(get: &dyn Fn(&str) -> Option<String>, name: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    collect(get, name, &mut out, 0);
    out
}

fn collect(get: &dyn Fn(&str) -> Option<String>, name: &str, out: &mut Vec<(String, String)>, depth: u32) {
    if depth > 8 {
        return;
    }
    let Some(line) = get(&format!("profile.{name}")) else {
        return;
    };
    for token in line.split_whitespace() {
        if let Some(parent) = token.strip_prefix("profile.") {
            collect(get, parent, out, depth + 1);
            continue;
        }
        let (key, value) = match token.split_once('=') {
            Some((k, v)) => (k.to_owned(), v.to_owned()),
            None => match token.strip_prefix('!') {
                Some(k) => (k.to_owned(), "false".to_owned()),
                None => (token.to_owned(), "true".to_owned()),
            },
        };
        match out.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = value,
            None => out.push((key, value)),
        }
    }
}

/// The text of a pack's options file for a quality level: a header the game recognises as its own
/// and one `NAME=value` per line, plus `extra` choices that win over the profile.
pub fn options_file(level: &str, options: &[(String, String)]) -> String {
    let mut text = format!("{MARKER} {level}\n");
    for (k, v) in options {
        text.push_str(&format!("{k}={v}\n"));
    }
    text
}

/// The first line of an options file the game wrote; one without it is the player's own.
pub const MARKER: &str = "# belumi-quality";

/// Whether an options file may be replaced: absent, empty or written by the game.
pub fn may_replace(existing: Option<&str>) -> bool {
    existing.is_none_or(|t| t.trim().is_empty() || t.starts_with(MARKER))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solas(key: &str) -> Option<String> {
        Some(
            match key {
                "profile.LOW" => "!LPV_FOG !BLOOM !VL WATER_NORMALS=0 shadowMapResolution=1024",
                "profile.MEDIUM" => "profile.LOW BLOOM VL WATER_NORMALS=3 VL_SAMPLES=7",
                "profile.HIGH" => "profile.MEDIUM LPV_FOG shadowMapResolution=2048",
                _ => return None,
            }
            .to_owned(),
        )
    }

    fn value<'a>(o: &'a [(String, String)], k: &str) -> Option<&'a str> {
        o.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }

    #[test]
    fn a_profile_builds_on_the_ones_it_names() {
        let low = profile_options(&solas, "LOW");
        assert_eq!(value(&low, "VL"), Some("false"));
        assert_eq!(value(&low, "shadowMapResolution"), Some("1024"));
        let high = profile_options(&solas, "HIGH");
        assert_eq!(value(&high, "VL"), Some("true"));
        assert_eq!(value(&high, "VL_SAMPLES"), Some("7"));
        assert_eq!(value(&high, "shadowMapResolution"), Some("2048"));
        assert_eq!(value(&high, "LPV_FOG"), Some("true"));
        assert_eq!(high.iter().filter(|(k, _)| k == "BLOOM").count(), 1);
        assert!(profile_options(&solas, "NOPE").is_empty());
    }

    #[test]
    fn the_options_file_is_marked_and_only_the_games_own_may_be_replaced() {
        let text = options_file("LOW", &[("VL".into(), "false".into())]);
        assert!(text.starts_with(MARKER));
        assert!(text.contains("VL=false"));
        assert!(may_replace(None));
        assert!(may_replace(Some("  \n")));
        assert!(may_replace(Some(&text)));
        assert!(!may_replace(Some("VL=true\n")));
    }
}
