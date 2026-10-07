//! Harmonic order follows the Mixed In Key Camelot wheel:
//! https://mixedinkey.com/camelot-wheel/
use std::cmp::Ordering;

pub fn compare(a: &str, b: &str) -> Ordering {
    rank(a)
        .unwrap_or(24)
        .cmp(&rank(b).unwrap_or(24))
        .then_with(|| a.to_lowercase().cmp(&b.to_lowercase()))
}

pub(crate) fn rank(value: &str) -> Option<u8> {
    let s = value.trim().replace('♯', "#").replace('♭', "b");
    let lower = s.to_lowercase().replace("sharp", "#").replace("flat", "b");
    let compact: String = lower.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.len() >= 2 && compact.is_ascii() {
        let (number, suffix) = compact.split_at(compact.len() - 1);
        if let Ok(n @ 1..=12) = number.parse::<u8>() {
            return match suffix {
                "a" => Some((n - 1) * 2),
                "b" => Some((n - 1) * 2 + 1),
                "m" => Some(((n + 6) % 12) * 2),
                "d" => Some(((n + 6) % 12) * 2 + 1),
                _ => None,
            };
        }
    }
    let (root, minor) = if let Some(root) = compact
        .strip_suffix("minor")
        .or_else(|| compact.strip_suffix("min"))
    {
        (root, true)
    } else if let Some(root) = compact
        .strip_suffix("major")
        .or_else(|| compact.strip_suffix("maj"))
    {
        (root, false)
    } else if let Some(root) = compact.strip_suffix('m') {
        // Uppercase M is conventional major shorthand.
        (root, !s.ends_with('M'))
    } else {
        (compact.as_str(), false)
    };
    let pitch = match root {
        "c" | "b#" => 0,
        "c#" | "db" => 1,
        "d" => 2,
        "d#" | "eb" => 3,
        "e" | "fb" => 4,
        "f" | "e#" => 5,
        "f#" | "gb" => 6,
        "g" => 7,
        "g#" | "ab" => 8,
        "a" => 9,
        "a#" | "bb" => 10,
        "b" | "cb" => 11,
        _ => return None,
    };
    let wheel = if minor {
        [5, 12, 7, 2, 9, 4, 11, 6, 1, 8, 3, 10]
    } else {
        [8, 3, 10, 5, 12, 7, 2, 9, 4, 11, 6, 1]
    };
    Some((wheel[pitch] - 1) * 2 + u8::from(!minor))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_keys_and_enharmonic_names_follow_the_same_wheel() {
        let names = [
            "Abm", "B", "Ebm", "F#", "Bbm", "Db", "Fm", "Ab", "Cm", "Eb", "Gm", "Bb", "Dm", "F",
            "Am", "C", "Em", "G", "Bm", "D", "F#m", "A", "C#m", "E",
        ];
        for (i, name) in names.iter().enumerate() {
            assert_eq!(rank(name), Some(i as u8));
            assert_eq!(
                rank(&format!(
                    "{}{}",
                    i / 2 + 1,
                    if i % 2 == 0 { "A" } else { "B" }
                )),
                Some(i as u8)
            );
        }
        assert_eq!(rank("G♯ minor"), rank("Abm"));
        assert_eq!(rank("F Sharp Major"), rank("Gb"));
        assert_eq!(rank("1m"), rank("8A"));
        assert_eq!(rank("6d"), rank("1B"));
        assert_eq!(rank("CM"), rank("C major"));
        assert_eq!(rank("unknown"), None);
        assert_eq!(rank(""), None);
        assert_eq!(compare("2A", "10A"), Ordering::Less);
    }
}
