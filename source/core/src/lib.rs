//! Read-only analysis adapter. No network services or deck control dependencies.
use prolink_rekordbox::anlz::{AnlzFile, FourCc};
use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub struct Column {
    pub low: f32,
    pub mid: f32,
    pub high: f32,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Waveform {
    pub tag: String,
    pub columns: Vec<Column>,
    pub raw_columns: Vec<[u8; 3]>,
    pub samples_per_second: Option<u16>,
    pub normalization: u16,
}
#[derive(Debug, Serialize)]
pub struct Analysis {
    pub beats: Vec<BeatMark>,
    pub detail: Option<Waveform>,
    pub preview: Option<Waveform>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BeatMark {
    pub time: f64,
    pub beat_in_bar: u16,
}

/// PWV7 detail: low, mid, high (matched against Brickell iPad references).
/// PWV6 preview retains mid, high, low. Preserve raw values.
/// Detail uses the CDJ-3000's 7-bit height domain. Preview scaling is provisional.
pub fn decode(data: &[u8]) -> Result<Analysis, String> {
    // Prolink intentionally salvages truncated files. This upload API must instead
    // reject partial containers, so a successful response always means complete data.
    if data.len() < 12 {
        return Err("Truncated PMAI header".into());
    }
    let read = |i| u32::from_be_bytes(data[i..i + 4].try_into().unwrap()) as usize;
    let header = read(4);
    if header < 12 || header > data.len() || read(8) != data.len() {
        return Err("PMAI declared length disagrees with file".into());
    }
    let mut offset = header;
    while offset < data.len() {
        if data.len() - offset < 12 {
            return Err("Truncated tag header".into());
        }
        let h = read(offset + 4);
        let n = read(offset + 8);
        if h < 12 || n < h || n > data.len() - offset {
            return Err("Invalid tag bounds".into());
        }
        offset += n;
    }
    let file = AnlzFile::parse(data).map_err(|e| e.to_string())?;
    let mut result = Analysis {
        beats: file
            .beat_grid()
            .map(|grid| {
                grid.beats
                    .iter()
                    .map(|b| BeatMark {
                        time: f64::from(b.time) / 1000.0,
                        beat_in_bar: b.beat_number,
                    })
                    .collect()
            })
            .unwrap_or_default(),
        detail: None,
        preview: None,
    };
    for (fourcc, header_len, detail) in [(FourCc::PWV7, 24, true), (FourCc::PWV6, 20, false)] {
        for tag in file.tags(fourcc) {
            let raw = &tag.raw;
            let u32_at = |i| u32::from_be_bytes(raw[i..i + 4].try_into().unwrap());
            if raw.len() < header_len || u32_at(4) as usize != header_len {
                return Err("Unexpected three-band header size".into());
            }
            let stride = u32_at(12) as usize;
            let count = u32_at(16) as usize;
            if stride != 3 || count.checked_mul(3) != Some(tag.payload().len()) || count == 0 {
                return Err("Three-band entry count/stride disagrees with payload".into());
            }
            if detail && tag.payload().iter().any(|&b| b > 127) {
                return Err("PWV7 height exceeds supported 7-bit domain".into());
            }
            let normalization = if detail { 127 } else { 255 };
            let raw_columns: Vec<[u8; 3]> = tag.payload().as_chunks::<3>().0.to_vec();
            let columns = raw_columns
                .iter()
                .map(|c| Column {
                    low: c[if detail { 0 } else { 2 }] as f32 / normalization as f32,
                    mid: c[if detail { 1 } else { 0 }] as f32 / normalization as f32,
                    high: c[if detail { 2 } else { 1 }] as f32 / normalization as f32,
                })
                .collect();
            let wave = Waveform {
                tag: fourcc.as_str().unwrap().into(),
                columns,
                raw_columns,
                samples_per_second: detail.then_some(150),
                normalization,
            };
            let target = if detail {
                &mut result.detail
            } else {
                &mut result.preview
            };
            if target.is_some() {
                return Err("Duplicate three-band section".into());
            }
            *target = Some(wave);
        }
    }
    if result.detail.is_none() && result.preview.is_none() {
        return Err("No PWV6/PWV7 data in this file".into());
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(kind: &[u8; 4], stride: u32, count: u32, bytes: &[u8]) -> Vec<u8> {
        let h: u32 = if kind == b"PWV7" { 24 } else { 20 };
        let n = h + bytes.len() as u32;
        let mut b = b"PMAI".to_vec();
        b.extend(28u32.to_be_bytes());
        b.extend((28 + n).to_be_bytes());
        b.extend([0; 16]);
        b.extend(kind);
        b.extend(h.to_be_bytes());
        b.extend(n.to_be_bytes());
        b.extend(stride.to_be_bytes());
        b.extend(count.to_be_bytes());
        if h == 24 {
            b.extend(0x00960000u32.to_be_bytes());
        }
        b.extend(bytes);
        b
    }
    #[test]
    fn independent_bands_and_silence() {
        let b = fixture(b"PWV7", 3, 4, &[127, 0, 0, 0, 127, 0, 0, 0, 127, 0, 0, 0]);
        let c = decode(&b).unwrap().detail.unwrap().columns;
        assert_eq!(
            c[0],
            Column {
                low: 1.,
                mid: 0.,
                high: 0.
            }
        );
        assert_eq!(
            c[1],
            Column {
                low: 0.,
                mid: 1.,
                high: 0.
            }
        );
        assert_eq!(
            c[2],
            Column {
                low: 0.,
                mid: 0.,
                high: 1.
            }
        );
        assert_eq!(
            c[3],
            Column {
                low: 0.,
                mid: 0.,
                high: 0.
            }
        );
    }
    #[test]
    fn malformed_dimensions_and_truncation() {
        for b in [
            fixture(b"PWV7", 6, 1, &[1, 2, 3]),
            fixture(b"PWV7", 3, 2, &[1, 2, 3]),
            fixture(b"PWV7", 3, 1, &[128, 0, 0]),
        ] {
            assert!(decode(&b).is_err());
        }
        let b = fixture(b"PWV7", 3, 1, &[1, 2, 3]);
        for n in 0..b.len() {
            assert!(decode(&b[..n]).is_err(), "length {n}");
        }
    }
    #[test]
    fn preview_is_not_a_150hz_detail() {
        let a = decode(&fixture(b"PWV6", 3, 1, &[10, 20, 30])).unwrap();
        let w = a.preview.unwrap();
        assert_eq!(w.samples_per_second, None);
        assert_eq!(w.raw_columns[0], [10, 20, 30]);
        assert!(a.detail.is_none());
    }
    #[test]
    fn combined_detail_and_preview() {
        let mut detail = fixture(b"PWV7", 3, 2, &[11, 20, 0, 0, 0, 127]);
        let preview = fixture(b"PWV6", 3, 1, &[10, 20, 30]);
        detail.extend_from_slice(&preview[28..]);
        let length = detail.len() as u32;
        detail[8..12].copy_from_slice(&length.to_be_bytes());
        let a = decode(&detail).unwrap();
        assert_eq!(a.detail.unwrap().raw_columns, [[11, 20, 0], [0, 0, 127]]);
        assert_eq!(a.preview.unwrap().raw_columns, [[10, 20, 30]]);
    }
}
