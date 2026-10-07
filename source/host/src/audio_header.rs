//! Read-only file-header evidence; never converts or rewrites audio.
use serde_json::{Value, json};
use std::{
    io::{Read, Seek, SeekFrom},
    path::Path,
};
pub fn inspect(path: &Path) -> Option<Value> {
    let mut file = std::fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let mut header = [0; 12];
    file.read_exact(&mut header).ok()?;
    if &header[..4] != b"RIFF" || &header[8..] != b"WAVE" {
        return None;
    }
    let mut result = json!({"container":"WAV","fileBytes":length});
    let mut offset = 12u64;
    for _ in 0..4096 {
        if offset.checked_add(8)? > length {
            break;
        }
        file.seek(SeekFrom::Start(offset)).ok()?;
        let mut chunk = [0; 8];
        file.read_exact(&mut chunk).ok()?;
        let size = u64::from(u32::from_le_bytes(chunk[4..].try_into().ok()?));
        if offset.checked_add(8)?.checked_add(size)? > length {
            return None;
        }
        if &chunk[..4] == b"fmt " && size >= 16 {
            let mut fmt = [0; 16];
            file.read_exact(&mut fmt).ok()?;
            let u16at = |i| u16::from_le_bytes([fmt[i], fmt[i + 1]]);
            result["encoding"] = json!(u16at(0));
            result["channels"] = json!(u16at(2));
            result["sampleRate"] = json!(u32::from_le_bytes(fmt[4..8].try_into().ok()?));
            result["byteRate"] = json!(u32::from_le_bytes(fmt[8..12].try_into().ok()?));
            result["blockAlign"] = json!(u16at(12));
            result["sampleDepth"] = json!(u16at(14));
        } else if &chunk[..4] == b"data" {
            result["dataOffset"] = json!(offset + 8);
            result["dataBytes"] = json!(size);
        }
        if !result["encoding"].is_null() && !result["dataOffset"].is_null() {
            return Some(result);
        }
        offset = offset
            .checked_add(8)?
            .checked_add(size)?
            .checked_add(size % 2)?;
    }
    None
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pcm_header_and_odd_padded_chunks_are_read_without_changing_file() {
        let path = std::env::temp_dir().join(format!("olc-header-{}.wav", std::process::id()));
        let mut bytes = b"RIFF\0\0\0\0WAVEJUNK\x01\0\0\0x\0fmt \x10\0\0\0".to_vec();
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&44100u32.to_le_bytes());
        bytes.extend_from_slice(&264600u32.to_le_bytes());
        bytes.extend_from_slice(&6u16.to_le_bytes());
        bytes.extend_from_slice(&24u16.to_le_bytes());
        bytes.extend_from_slice(b"data\x06\0\0\0abcdef");
        let len = bytes.len() as u32 - 8;
        bytes[4..8].copy_from_slice(&len.to_le_bytes());
        std::fs::write(&path, &bytes).unwrap();
        let report = inspect(&path).unwrap();
        assert_eq!(report["dataOffset"], 54);
        assert_eq!(report["sampleDepth"], 24);
        assert_eq!(report["sampleRate"], 44100);
        assert_eq!(report["blockAlign"], 6);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        std::fs::remove_file(path).unwrap();
    }
}
