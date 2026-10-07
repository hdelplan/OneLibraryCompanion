//! Map native Rekordbox phrase boundaries through the exported beat grid.
use prolink_rekordbox::anlz::{Phrase, SongStructure, TrackMood};
use serde_json::{Value, json};

pub fn mood_label(mood: TrackMood) -> Option<&'static str> {
    match mood {
        TrackMood::LOW => Some("Low"),
        TrackMood::MID => Some("Mid"),
        TrackMood::HIGH => Some("High"),
        _ => None,
    }
}

pub fn segments(structure: &SongStructure, beats: &[f64], duration: f64) -> Vec<Value> {
    if structure.len_entry_bytes != 24 || !duration.is_finite() || duration <= 0.0 {
        return vec![];
    }
    structure.phrases.iter().enumerate().filter_map(|(i, phrase)| {
        let end_beat = structure.phrases.get(i + 1).map_or(structure.end_beat, |p| p.beat);
        let start = *beats.get(usize::from(phrase.beat.checked_sub(1)?))?;
        let end = beats.get(usize::from(end_beat.checked_sub(1)?))?.min(duration);
        if !start.is_finite() || !end.is_finite() || start < 0.0 || end <= start { return None; }
        Some(json!({"start":start,"end":end,"label":phrase.label(structure.mood).unwrap_or("unknown"),"kind":phrase.kind,"color":phrase_color(phrase,structure.mood),"textColor":if structure.mood==TrackMood::HIGH {"#ffffff"} else {"#000000"}}))
    }).collect()
}

// Rekordbox mood/variant palette, numeric palette documented by Beat Link Util.java.
fn phrase_color(p: &Phrase, mood: TrackMood) -> String {
    let rgb = match mood {
        TrackMood::LOW => match p.kind {
            1 => (255, 170, 180),
            2..=4 => (165, 160, 255),
            5..=7 => (190, 160, 255),
            8 => (255, 250, 165),
            9 => (185, 225, 185),
            10 => (145, 160, 180),
            _ => (255, 255, 255),
        },
        TrackMood::MID => match p.kind {
            1 => (225, 70, 70),
            2 => (80, 110, 255),
            3 => (80, 85, 255),
            4 => (100, 80, 255),
            5 => (120, 80, 255),
            6 => (140, 80, 255),
            7 => (160, 80, 255),
            8 => (225, 215, 65),
            9 => (120, 195, 125),
            10 => (115, 130, 150),
            _ => (255, 255, 255),
        },
        TrackMood::HIGH => match p.kind {
            1 => {
                if p.k1 == 1 {
                    (200, 0, 0)
                } else {
                    (200, 50, 0)
                }
            }
            2 => {
                if p.k2 != 0 {
                    (90, 50, 255)
                } else if p.k3 == 0 {
                    (140, 50, 255)
                } else {
                    (105, 50, 255)
                }
            }
            3 => (155, 115, 45),
            5 => (15, 170, 0),
            6 => {
                if p.k1 == 1 {
                    (80, 135, 195)
                } else {
                    (95, 135, 175)
                }
            }
            _ => (255, 255, 255),
        },
        _ => (255, 255, 255),
    };
    format!("#{:02x}{:02x}{:02x}", rgb.0, rgb.1, rgb.2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use prolink_rekordbox::anlz::{Content, FourCc};
    fn structure(masked: bool) -> SongStructure {
        let mut body = vec![0, 0, 0, 24, 0, 2, 0, 1, 0, 0, 0, 0, 0, 0, 0, 9, 0, 0, 0, 0];
        for (index, beat, kind) in [(1u16, 1u16, 1u16), (2, 5, 5)] {
            let mut entry = vec![0; 24];
            entry[0..2].copy_from_slice(&index.to_be_bytes());
            entry[2..4].copy_from_slice(&beat.to_be_bytes());
            entry[4..6].copy_from_slice(&kind.to_be_bytes());
            body.extend(entry);
        }
        if masked {
            let key: [u8; 19] = [
                0xcb, 0xe1, 0xee, 0xfa, 0xe5, 0xee, 0xad, 0xee, 0xe9, 0xd2, 0xe9, 0xeb, 0xe1, 0xe9,
                0xf3, 0xe8, 0xe9, 0xf4, 0xe1,
            ];
            for (i, b) in body[6..].iter_mut().enumerate() {
                *b ^= key[i % 19].wrapping_add(2);
            }
        }
        match Content::parse(FourCc::PSSI, &body).unwrap() {
            Content::SongStructure(s) => s,
            _ => panic!("wrong tag"),
        }
    }
    #[test]
    fn masked_and_plain_phrases_use_one_based_beat_times() {
        let beats = [0.1, 0.6, 1.1, 1.6, 2.1, 2.6, 3.1, 3.6, 4.1];
        for masked in [false, true] {
            let result = segments(&structure(masked), &beats, 4.0);
            assert_eq!(result.len(), 2);
            assert_eq!(
                result[0],
                json!({"start":0.1,"end":2.1,"label":"intro","kind":1,"color":"#c83200","textColor":"#ffffff"})
            );
            assert_eq!(result[1]["end"], 4.0);
            assert_eq!(result[1]["label"], "chorus");
        }
    }
    #[test]
    fn exported_mood_values_match_the_palette_names() {
        assert_eq!(mood_label(TrackMood(1)), Some("High"));
        assert_eq!(mood_label(TrackMood(2)), Some("Mid"));
        assert_eq!(mood_label(TrackMood(3)), Some("Low"));
        assert_eq!(mood_label(TrackMood(0)), None);
        assert_eq!(mood_label(TrackMood(4)), None);
        for masked in [false, true] {
            assert_eq!(mood_label(structure(masked).mood), Some("High"));
        }
    }
    #[test]
    fn palette_preserves_mood_and_high_energy_variants() {
        let mut phrase = structure(false).phrases[0];
        assert_eq!(phrase_color(&phrase, TrackMood::LOW), "#ffaab4");
        assert_eq!(phrase_color(&phrase, TrackMood::MID), "#e14646");
        phrase.k1 = 1;
        assert_eq!(phrase_color(&phrase, TrackMood::HIGH), "#c80000");
        phrase.kind = 2;
        phrase.k2 = 0;
        phrase.k3 = 0;
        assert_eq!(phrase_color(&phrase, TrackMood::HIGH), "#8c32ff");
        phrase.k3 = 1;
        assert_eq!(phrase_color(&phrase, TrackMood::HIGH), "#6932ff");
        phrase.k2 = 1;
        assert_eq!(phrase_color(&phrase, TrackMood::HIGH), "#5a32ff");
        phrase.kind = 5;
        assert_eq!(phrase_color(&phrase, TrackMood::HIGH), "#0faa00");
    }
    #[test]
    fn missing_grid_and_invalid_boundaries_do_not_invent_phrases() {
        assert!(segments(&structure(false), &[], 10.0).is_empty());
        assert!(segments(&structure(false), &[0.0; 9], 10.0).is_empty());
        let mut s = structure(false);
        s.len_entry_bytes = 25;
        assert!(segments(&s, &[0.0; 9], 10.0).is_empty());
    }
}
