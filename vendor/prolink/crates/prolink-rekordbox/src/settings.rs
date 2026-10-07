// SPDX-License-Identifier: GPL-3.0-only

//! `PIONEER/*SETTING*.DAT` — the "My Settings" files a player adopts.
//!
//! LCD brightness, jog tension, auto-cue level, whether the key display is
//! alphanumeric or classic. A deck applies these from a locally inserted USB,
//! and also from a **peer's** medium over LINK — and that path does not read
//! the file over NFS at all. The requesting deck mounts the export, touches
//! nothing, and asks over UDP 50002 instead; the owner reads its own copy and
//! returns the bytes inline (F38). So a server has to read this file itself.
//!
//! The container is uniform across the four variants found on a real medium,
//! and **little-endian** — unlike the big-endian ANLZ files beside it:
//!
//! ```text
//! 0x00  u32       header length, always 96
//! 0x04  char[32]  brand      "PIONEER" / "PIONEER DJ" / "PioneerDJ"
//! 0x24  char[32]  creator    "rekordbox"
//! 0x44  char[32]  version    "0.001" / "7.1.0" / "1.000"
//! 0x64  u32       payload length
//! 0x68  payload
//!       u16       CRC-16/XMODEM checksum, then two padding bytes
//! ```
//!
//! In `MYSETTING.DAT`, `DEVSETTING.DAT` and `DJMMYSETTING.DAT` the payload opens
//! with the constant `0x12345678` and a second word, and the settings bytes
//! follow at `0x70`. `MYSETTING2.DAT` does not carry the magic, so its first
//! eight bytes are settings data rather than a header — which is why
//! [`SettingsFile::settings`] refuses to guess for it rather than returning
//! eight meaningless bytes.
//!
//! # What is served and what is not
//!
//! Only `MYSETTING.DAT` has ever been seen on the wire. Its 32 settings bytes
//! are what a type-`0x36` reply carries, with the two leading words byte-swapped
//! to big-endian — that swap is the wire layer's business, and this module hands
//! over the bytes as the medium holds them. Nothing in the `0x35` request
//! obviously selects between the four variants, which stays an open question.
//!
//! The settings bytes themselves are not interpreted here. They look like
//! `0x80`-based enumerations, and serving needs only to pass them on. Turn on
//! the `settings-detail` feature, which adds a `detail` function handing them
//! to `rekordcrate` for naming.
//!
//! # No captured file was available
//!
//! There was no `*SETTING*.DAT` on the machine this was written on. The layout
//! comes from the project's notes and `rekordcrate`, and the tests build files
//! from that layout. In particular the checksum is **computed and reported, not
//! enforced**: a rule that rejects a real file because our CRC convention is
//! subtly wrong would be worse than one that reports a mismatch a caller can
//! ignore.

use crate::error::{Error, Result};

/// Path of the file a type-`0x36` reply carries, relative to the export root.
pub const MY_SETTING_PATH: &str = "PIONEER/MYSETTING.DAT";
/// Path of the second player-settings file.
pub const MY_SETTING2_PATH: &str = "PIONEER/MYSETTING2.DAT";
/// Path of the device-settings file.
pub const DEV_SETTING_PATH: &str = "PIONEER/DEVSETTING.DAT";
/// Path of the mixer-settings file.
pub const DJM_SETTING_PATH: &str = "PIONEER/DJMMYSETTING.DAT";

/// Declared header length; 96 on every variant.
pub const HEADER_LEN: u32 = 0x60;

/// Offset of the payload length field.
pub const OFS_PAYLOAD_LEN: usize = 0x64;

/// Offset of the payload.
pub const OFS_PAYLOAD: usize = 0x68;

/// Constant the payload opens with, on every variant but `MYSETTING2.DAT`.
///
/// The same value appears big-endian in the type-`0x36` reply, which is what
/// ties the file to the wire.
pub const PAYLOAD_MAGIC: u32 = 0x1234_5678;

/// Bytes of the payload the magic and the word after it occupy.
pub const PAYLOAD_PREFIX_LEN: usize = 8;

/// Settings bytes a type-`0x36` reply carries.
pub const WIRE_SETTINGS_LEN: usize = 32;

/// Which of the four settings files this is, deduced from the payload length.
///
/// A newtype over the length rather than an enum keyed on the file name,
/// because the name is not in the file and a caller may have the bytes without
/// it. The lengths are distinct, which is the only reason this works.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct SettingsKind(pub u32);

impl SettingsKind {
    /// `DEVSETTING.DAT`: an 8-byte prefix and 24 settings bytes.
    pub const DEV_SETTING: Self = Self(32);
    /// `MYSETTING.DAT`: an 8-byte prefix and 32 settings bytes.
    pub const MY_SETTING: Self = Self(40);
    /// `DJMMYSETTING.DAT`: an 8-byte prefix and 44 settings bytes.
    pub const DJM_MY_SETTING: Self = Self(52);

    /// A name for logs, or `None` for a payload length never seen.
    ///
    /// `MYSETTING.DAT` and `MYSETTING2.DAT` share a payload length of 40 and
    /// are told apart by the magic, not by this, so both are reported here.
    pub fn name(self) -> Option<&'static str> {
        Some(match self {
            Self::DEV_SETTING => "devsetting",
            Self::MY_SETTING => "mysetting or mysetting2",
            Self::DJM_MY_SETTING => "djmmysetting",
            _ => return None,
        })
    }
}

impl std::fmt::Debug for SettingsKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.name() {
            Some(name) => f.write_str(name),
            None => write!(f, "SettingsKind({})", self.0),
        }
    }
}

/// One parsed settings file.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SettingsFile {
    /// `0x04`. `"PIONEER"`, `"PIONEER DJ"` or `"PioneerDJ"`.
    pub brand: String,
    /// `0x24`. `"rekordbox"`.
    pub creator: String,
    /// `0x44`. A version string, whose numbering varies by variant.
    pub version: String,
    /// The declared payload, from `0x68`.
    pub payload: Vec<u8>,
    /// The stored checksum, if the file is long enough to carry one.
    pub stored_checksum: Option<u16>,
}

impl SettingsFile {
    /// Parse a settings file.
    ///
    /// Strict about the framing: a declared payload running past the buffer is
    /// an error rather than being silently clipped, because handing a real deck
    /// a truncated settings block is worse than failing the request.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let buffer_len = u64::try_from(data.len()).unwrap_or(u64::MAX);
        let need = u64::try_from(OFS_PAYLOAD).unwrap_or(u64::MAX);
        if buffer_len < need {
            return Err(Error::truncated(0, need, buffer_len));
        }
        let header_len = u32_at(data, 0).unwrap_or(0);
        if header_len != HEADER_LEN {
            return Err(Error::malformed(
                0,
                format!("header length {header_len:#x}, expected {HEADER_LEN:#x}"),
            ));
        }
        let payload_len = u32_at(data, OFS_PAYLOAD_LEN).unwrap_or(0);
        let declared = usize::try_from(payload_len).unwrap_or(usize::MAX);
        let end = OFS_PAYLOAD.saturating_add(declared);
        let Some(payload) = data.get(OFS_PAYLOAD..end) else {
            return Err(Error::truncated(
                u64::try_from(OFS_PAYLOAD).unwrap_or(u64::MAX),
                u64::from(payload_len),
                buffer_len.saturating_sub(need),
            ));
        };

        Ok(Self {
            brand: fixed_text(data, 0x04),
            creator: fixed_text(data, 0x24),
            version: fixed_text(data, 0x44),
            payload: payload.to_vec(),
            stored_checksum: u16_at(data, end),
        })
    }

    /// Which variant this is, by payload length.
    pub fn kind(&self) -> SettingsKind {
        SettingsKind(u32::try_from(self.payload.len()).unwrap_or(0))
    }

    /// Whether the payload opens with [`PAYLOAD_MAGIC`].
    ///
    /// False for `MYSETTING2.DAT` and true for the other three.
    pub fn has_magic(&self) -> bool {
        self.payload
            .get(..4)
            .and_then(|raw| <[u8; 4]>::try_from(raw).ok())
            .map(u32::from_le_bytes)
            == Some(PAYLOAD_MAGIC)
    }

    /// The settings bytes, past the magic and the word after it.
    ///
    /// Empty when the payload carries no magic, which is the case for
    /// `MYSETTING2.DAT` — better than returning its first eight bytes as though
    /// they meant something.
    pub fn settings(&self) -> &[u8] {
        if self.has_magic() {
            self.payload.get(PAYLOAD_PREFIX_LEN..).unwrap_or_default()
        } else {
            &[]
        }
    }

    /// The bytes a type-`0x36` reply carries, or empty if there are none.
    ///
    /// The wire layer byte-swaps the two leading words on the way out; that is
    /// not done here.
    pub fn wire_settings(&self) -> &[u8] {
        let settings = self.settings();
        settings
            .get(..WIRE_SETTINGS_LEN.min(settings.len()))
            .unwrap_or_default()
    }

    /// CRC-16/XMODEM over the payload, which is what the file should store.
    ///
    /// `DJMMYSETTING.DAT` is the exception: its checksum covers everything
    /// before it, including the header. That is reported by `rekordcrate` and
    /// has not been checked against a file here, so [`SettingsFile::kind`]
    /// decides which rule applies and a caller can compute the other.
    pub fn computed_checksum(&self) -> u16 {
        crc16_xmodem(&self.payload)
    }

    /// Whether the stored checksum matches [`SettingsFile::computed_checksum`].
    ///
    /// `None` when the file carries no checksum, and **not enforced by the
    /// parser**: no captured settings file was available to confirm which bytes
    /// the CRC covers on each variant, so a mismatch is reported rather than
    /// treated as corruption.
    pub fn checksum_matches(&self) -> Option<bool> {
        Some(self.stored_checksum? == self.computed_checksum())
    }
}

/// The named options behind the settings bytes — jog ring brightness, auto cue
/// level, tempo range and the rest.
///
/// Requires the `settings-detail` feature, which exists for this one thing:
/// `rekordcrate` has done the work of mapping the `0x80`-based enumerations to
/// the labels on a deck's screen, and nothing else has. It is off by default
/// because nothing in the Pro DJ Link protocol needs it — a player asks for the
/// bytes and formats them itself — and because `rekordcrate` pins `binrw` 0.14
/// where the rest of this workspace is on 0.15.
///
/// Takes the whole file rather than a parsed [`SettingsFile`], deliberately:
/// this is a **second, independent read** of the same bytes, so a disagreement
/// about the container is visible rather than hidden behind a shared parse.
/// `None` when `rekordcrate` refuses the file, which it does more readily than
/// this module — it asserts on several fields whose meaning is unknown.
#[cfg(feature = "settings-detail")]
#[cfg_attr(docsrs, doc(cfg(feature = "settings-detail")))]
pub fn detail(data: &[u8]) -> Option<rekordcrate::setting::Setting> {
    use binrw_014::BinRead as _;

    rekordcrate::setting::Setting::read(&mut binrw_014::io::Cursor::new(data)).ok()
}

/// The type-`0x36` settings bytes of a file, or empty for anything unusable.
///
/// Empty rather than an error: a medium with no settings on it is normal, and
/// the reply has a representation for that.
pub fn wire_settings(data: &[u8]) -> Vec<u8> {
    SettingsFile::parse(data)
        .map(|file| file.wire_settings().to_vec())
        .unwrap_or_default()
}

/// CRC-16/XMODEM: polynomial `0x1021`, initial value zero, no reflection.
fn crc16_xmodem(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for byte in data {
        crc ^= u16::from(*byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 == 0 {
                crc << 1
            } else {
                (crc << 1) ^ 0x1021
            };
        }
    }
    crc
}

fn fixed_text(data: &[u8], at: usize) -> String {
    let field = data.get(at..at.saturating_add(32)).unwrap_or_default();
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(field.get(..end).unwrap_or_default()).into_owned()
}

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    let raw: [u8; 2] = data.get(at..at.checked_add(2)?)?.try_into().ok()?;
    Some(u16::from_le_bytes(raw))
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    let raw: [u8; 4] = data.get(at..at.checked_add(4)?)?.try_into().ok()?;
    Some(u32::from_le_bytes(raw))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a settings file with the container the four variants share.
    ///
    /// Synthetic. No captured `*SETTING*.DAT` was available; this exercises the
    /// reader against the documented layout, which is not the same thing as
    /// exercising it against rekordbox.
    fn build(brand: &str, version: &str, payload: &[u8]) -> Vec<u8> {
        let mut out = HEADER_LEN.to_le_bytes().to_vec();
        for (at, text) in [(0x04, brand), (0x24, "rekordbox"), (0x44, version)] {
            out.resize(at, 0);
            out.extend_from_slice(text.as_bytes());
        }
        out.resize(OFS_PAYLOAD_LEN, 0);
        out.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_le_bytes());
        out.extend_from_slice(payload);
        out.extend_from_slice(&crc16_xmodem(payload).to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out
    }

    /// A `MYSETTING.DAT` payload: magic, a word, then 32 settings bytes.
    fn my_setting_payload() -> Vec<u8> {
        let mut payload = PAYLOAD_MAGIC.to_le_bytes().to_vec();
        payload.extend_from_slice(&1u32.to_le_bytes());
        payload.extend((0..WIRE_SETTINGS_LEN).map(|index| 0x80 | u8::try_from(index).unwrap()));
        payload
    }

    #[test]
    fn a_mysetting_file_parses_into_its_thirty_two_settings_bytes() {
        let raw = build("PIONEER", "0.001", &my_setting_payload());
        let parsed = SettingsFile::parse(&raw).unwrap();
        assert_eq!(parsed.brand, "PIONEER");
        assert_eq!(parsed.creator, "rekordbox");
        assert_eq!(parsed.version, "0.001");
        assert_eq!(parsed.kind(), SettingsKind::MY_SETTING);
        assert!(parsed.has_magic());
        assert_eq!(parsed.settings().len(), WIRE_SETTINGS_LEN);
        assert_eq!(parsed.settings().first(), Some(&0x80));
        // The 32 bytes start at 0x70: the payload at 0x68 plus the 8-byte prefix.
        assert_eq!(parsed.wire_settings(), raw.get(0x70..0x90).unwrap());
    }

    #[test]
    fn mysetting2_has_no_magic_so_its_settings_are_not_guessed_at() {
        // Same payload length, no magic: its first eight bytes are data.
        let payload = vec![0x81u8; 40];
        let raw = build("PIONEER", "1.000", &payload);
        let parsed = SettingsFile::parse(&raw).unwrap();
        assert!(!parsed.has_magic());
        assert_eq!(
            parsed.settings(),
            &[] as &[u8],
            "returning the first eight bytes would be inventing a header"
        );
        assert_eq!(parsed.payload.len(), 40, "the payload is still available");
    }

    #[test]
    fn the_variants_are_told_apart_by_payload_length() {
        for (length, kind) in [
            (32, SettingsKind::DEV_SETTING),
            (40, SettingsKind::MY_SETTING),
            (52, SettingsKind::DJM_MY_SETTING),
        ] {
            let mut payload = PAYLOAD_MAGIC.to_le_bytes().to_vec();
            payload.resize(length, 0);
            let raw = build("PIONEER DJ", "7.1.0", &payload);
            assert_eq!(SettingsFile::parse(&raw).unwrap().kind(), kind);
        }
    }

    #[test]
    fn the_checksum_is_reported_rather_than_enforced() {
        let mut raw = build("PIONEER", "0.001", &my_setting_payload());
        let parsed = SettingsFile::parse(&raw).unwrap();
        assert_eq!(parsed.checksum_matches(), Some(true));

        // Corrupt it: the file must still parse, and say so.
        let at = OFS_PAYLOAD + 40;
        raw.splice(at..at + 2, [0xff, 0xff]);
        let damaged = SettingsFile::parse(&raw).unwrap();
        assert_eq!(damaged.checksum_matches(), Some(false));
        assert_eq!(damaged.wire_settings().len(), WIRE_SETTINGS_LEN);
    }

    #[test]
    fn crc16_xmodem_matches_its_published_check_value() {
        // The CRC catalogue's check value for CRC-16/XMODEM.
        assert_eq!(crc16_xmodem(b"123456789"), 0x31c3);
    }

    #[test]
    fn a_wrong_header_length_is_rejected() {
        let mut raw = build("PIONEER", "0.001", &my_setting_payload());
        raw.splice(0..4, 0x40u32.to_le_bytes());
        assert!(matches!(
            SettingsFile::parse(&raw),
            Err(Error::Malformed { .. })
        ));
    }

    #[test]
    fn a_payload_running_past_the_buffer_is_rejected_not_clipped() {
        let mut raw = build("PIONEER", "0.001", &my_setting_payload());
        raw.splice(OFS_PAYLOAD_LEN..OFS_PAYLOAD_LEN + 4, 4096u32.to_le_bytes());
        let error = SettingsFile::parse(&raw).unwrap_err();
        assert!(error.is_truncated(), "got {error:?}");
    }

    #[test]
    fn a_short_file_is_truncated_rather_than_malformed() {
        let error = SettingsFile::parse(&[0; 16]).unwrap_err();
        assert!(error.is_truncated(), "got {error:?}");
    }

    #[test]
    fn an_unusable_file_yields_no_wire_settings_rather_than_an_error() {
        assert!(wire_settings(b"not a settings file").is_empty());
        assert_eq!(
            wire_settings(&build("PIONEER", "0.001", &my_setting_payload())).len(),
            WIRE_SETTINGS_LEN
        );
    }

    /// The closest thing to a captured file available: `rekordcrate` writes a
    /// `MYSETTING.DAT` from its own model of the format, and this module reads
    /// it. Two independent implementations of the container, which is the only
    /// cross-check possible without hardware.
    #[cfg(feature = "settings-detail")]
    #[test]
    fn a_file_another_implementation_wrote_reads_back_the_same_way() {
        use binrw_014::BinWrite as _;

        let written = rekordcrate::setting::Setting::default_mysetting();
        let mut raw = binrw_014::io::Cursor::new(Vec::new());
        written.write_args(&mut raw, (false,)).unwrap();
        let raw = raw.into_inner();

        let parsed = SettingsFile::parse(&raw).unwrap();
        assert_eq!(parsed.brand, "PIONEER");
        assert_eq!(parsed.creator, "rekordbox");
        assert_eq!(parsed.kind(), SettingsKind::MY_SETTING);
        assert!(parsed.has_magic(), "the payload magic must be found");
        assert_eq!(parsed.settings().len(), WIRE_SETTINGS_LEN);
        assert_eq!(
            parsed.checksum_matches(),
            Some(true),
            "our CRC-16/XMODEM must agree with theirs"
        );
        assert_eq!(detail(&raw), Some(written));
    }
}
