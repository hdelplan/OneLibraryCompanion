use prolink_rekordbox::AnlzFile;
use serde_json::{Value, json};
pub fn collect(anlz: &AnlzFile, cues: &mut Vec<Value>) {
    for list in anlz.cue_lists() {
        for cue in &list.cues {
            if matches!(cue.cue_type.0, 1 | 2) {
                insert(cues, cue.hot_cue, cue.time, None);
            }
        }
    }
    for list in anlz.extended_cue_lists() {
        for cue in &list.cues {
            if !matches!(cue.cue_type.0, 1 | 2) {
                continue;
            }
            let rgb = (
                cue.hot_cue_color_red,
                cue.hot_cue_color_green,
                cue.hot_cue_color_blue,
            );
            let color =
                (rgb != (0, 0, 0)).then(|| format!("#{:02x}{:02x}{:02x}", rgb.0, rgb.1, rgb.2));
            insert(cues, cue.hot_cue, cue.time, color);
        }
    }
}
fn insert(cues: &mut Vec<Value>, hot: u32, millis: u32, color: Option<String>) {
    if hot > 8 {
        return;
    }
    let time = f64::from(millis) / 1000.0;
    let label = if hot == 0 {
        "CUE".to_owned()
    } else {
        char::from(b'A' + hot as u8 - 1).to_string()
    };
    let value = json!({"time":time,"hot":hot,"label":label,
        "color":color.unwrap_or_else(|| if hot == 0 {"#ff4040"} else {"#28d957"}.into())});
    if let Some(existing) = cues
        .iter_mut()
        .find(|c| c["hot"] == hot && (hot > 0 || c["time"] == time))
    {
        *existing = value;
    } else {
        cues.push(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extended_colors_replace_basic_cues_without_losing_memory_points() {
        let mut cues = vec![];
        insert(&mut cues, 1, 1000, None);
        insert(&mut cues, 0, 1000, None);
        insert(&mut cues, 1, 1000, Some("#1a00ff".into()));
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0]["color"], "#1a00ff");
        assert_eq!(cues[0]["label"], "A");
        insert(&mut cues, 8, 2000, Some("#1aff00".into()));
        assert_eq!(cues[2]["label"], "H");
        insert(&mut cues, 99, 0, None);
        assert_eq!(cues.len(), 3);
    }
}

/// A cued deck with no trustworthy fine signal can use a unique saved marker
/// close to its reported beat. Ambiguous markers remain coarse, never guessed.
pub fn saved_position(
    status: prolink::monitor::PlayerStatus,
    grid: &[f64],
    cues: &[Value],
) -> Option<f64> {
    if status.play_state.0 != 6 || status.is_playing {
        return None;
    }
    let index = status.beat_number?.saturating_sub(1) as usize;
    let anchor = *grid.get(index)?;
    let period = grid.get(index + 1)? - anchor;
    if period <= 0.0 {
        return None;
    }
    let mut candidates: Vec<f64> = cues
        .iter()
        .filter_map(|c| c["time"].as_f64())
        .filter(|t| (*t - anchor).abs() <= period * 0.45)
        .collect();
    candidates.sort_by(f64::total_cmp);
    candidates.dedup_by(|a, b| (*a - *b).abs() < 0.002);
    (candidates.len() == 1).then(|| candidates[0])
}

#[cfg(test)]
mod position_tests {
    use super::*;
    #[test]
    fn unique_cue_and_ambiguous_markers() {
        let packet = prolink_proto::status::CdjStatus::builder()
            .play_state(6)
            .playing(false)
            .build();
        let mut status = prolink::monitor::PlayerStatus::from_packet(&packet);
        status.beat_number = Some(2);
        let grid = [15.16, 15.66, 16.16];
        assert_eq!(
            saved_position(status, &grid, &[json!({"time":15.66})]),
            Some(15.66)
        );
        assert_eq!(
            saved_position(
                status,
                &grid,
                &[json!({"time":15.66}), json!({"time":15.77})]
            ),
            None
        );
        status.play_state = prolink::monitor::PlayState(5);
        assert_eq!(
            saved_position(status, &grid, &[json!({"time":15.66})]),
            None
        );
    }
}
