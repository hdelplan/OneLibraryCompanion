//! Synthetic reproduction of a real export's 284-slot playlist-entry page.
//! No private track or playlist data is included.
use prolink_rekordbox::{PageType, Pdb, pdb::PlaylistEntryRow};
fn word(data: &mut [u8], at: usize, value: u32) {
    data[at..at + 4].copy_from_slice(&value.to_le_bytes());
}
fn playlist_page() -> Vec<u8> {
    let mut data = vec![0; 8192];
    word(&mut data, 4, 4096);
    word(&mut data, 8, 1);
    word(&mut data, 28, 8);
    word(&mut data, 36, 1);
    word(&mut data, 40, 1);
    word(&mut data, 4096 + 4, 1);
    word(&mut data, 4096 + 8, 8);
    // Two packed counts occupy three bytes. The low 13 bits are 284 slots.
    word(&mut data, 4096 + 24, 284 | (284 << 13) | (0x24 << 24));
    data[4096 + 34..4096 + 36].copy_from_slice(&43u16.to_le_bytes());
    for index in 0..284usize {
        let start = 4096 + 40 + index * 12;
        word(&mut data, start, index as u32 + 1);
        word(&mut data, start + 4, index as u32 + 1000);
        word(&mut data, start + 8, if index >= 189 { 99 } else { 98 });
        let end = 8192 - (index / 16) * 36;
        let slot = index % 16;
        data[end - 6 - slot * 2..end - 4 - slot * 2]
            .copy_from_slice(&((index * 12) as u16).to_le_bytes());
        let mask = u16::from_le_bytes(data[end - 4..end - 2].try_into().unwrap());
        data[end - 4..end - 2].copy_from_slice(&(mask | (1 << slot)).to_le_bytes());
    }
    data
}
#[test]
fn packed_row_count_recovers_playlist_entries_beyond_the_low_byte() {
    let data = playlist_page();
    let pdb = Pdb::new(&data).unwrap();
    let header = pdb.pages(PageType::PLAYLIST_ENTRIES)[0].1;
    assert_eq!(header.num_rows(), 284);
    let rows = pdb.rows::<PlaylistEntryRow>();
    assert_eq!(rows.len(), 284);
    assert_eq!(rows.iter().filter(|r| r.playlist_id == 99).count(), 95);
    assert_eq!(rows.last().unwrap().track_id, 1283);
}
#[test]
fn deleted_rows_stay_deleted_with_the_full_packed_count() {
    let mut data = playlist_page();
    let index = 200;
    let end = 8192 - (index / 16) * 36;
    let mask = u16::from_le_bytes(data[end - 4..end - 2].try_into().unwrap());
    data[end - 4..end - 2].copy_from_slice(&(mask & !(1 << (index % 16))).to_le_bytes());
    let rows = Pdb::new(&data).unwrap().rows::<PlaylistEntryRow>();
    assert_eq!(rows.len(), 283);
    assert_eq!(rows.iter().filter(|r| r.playlist_id == 99).count(), 94);
    assert!(!rows.iter().any(|r| r.track_id == 1200));
}
