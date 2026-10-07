//! Synthetic extension pages: independent byte layout, no private library data.
use prolink_rekordbox::mytags::MyTags;
fn word(data: &mut [u8], at: usize, value: u32) {
    data[at..at + 4].copy_from_slice(&value.to_le_bytes());
}
fn definition(category: u32, id: u32, name: &str) -> Vec<u8> {
    let mut row = vec![0; 30];
    word(&mut row, 12, category);
    word(&mut row, 20, id);
    row[29] = 30;
    row.push((2 * (name.len() + 1) + 1) as u8);
    row.extend_from_slice(name.as_bytes());
    row
}
fn database() -> Vec<u8> {
    let mut data = vec![0; 3 * 4096];
    word(&mut data, 4, 4096);
    word(&mut data, 8, 2);
    for (i, kind) in [3, 4].into_iter().enumerate() {
        word(&mut data, 28 + i * 16, kind);
        word(&mut data, 36 + i * 16, (i + 1) as u32);
        word(&mut data, 40 + i * 16, (i + 1) as u32);
    }
    let relationship: Vec<u8> = [0u32, 42, 8, 3]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    for (page, kind, rows) in [
        (
            1,
            3,
            vec![definition(0, 7, "Mood"), definition(7, 8, "Warm")],
        ),
        (2, 4, vec![relationship]),
    ] {
        let start = page * 4096;
        word(&mut data, start + 4, page as u32);
        word(&mut data, start + 8, kind);
        data[start + 24] = rows.len() as u8;
        let end = start + 4096;
        data[end - 4..end - 2].copy_from_slice(&((1u16 << rows.len()) - 1).to_le_bytes());
        let mut offset = 0;
        for (i, row) in rows.into_iter().enumerate() {
            data[end - 6 - i * 2..end - 4 - i * 2].copy_from_slice(&(offset as u16).to_le_bytes());
            data[start + 40 + offset..start + 40 + offset + row.len()].copy_from_slice(&row);
            offset += row.len();
        }
    }
    data
}
#[test]
fn reads_categories_names_and_track_membership() {
    let tags = MyTags::parse(&database()).unwrap();
    assert_eq!(tags.categories[&7], "Mood");
    assert_eq!(tags.values[&8].name, "Warm");
    assert!(tags.track_tags[&42].contains(&8));
}
#[test]
fn corrupt_rows_fail_instead_of_becoming_empty_tags() {
    let mut data = database();
    data[4096 + 40 + 29] = 255;
    assert!(MyTags::parse(&data).is_err());
    let mut data = database();
    data[4096 + 24] = 255;
    data[4096 + 25] = 31; // Impossible 8191-slot footer within one 4096-byte page.
    assert!(MyTags::parse(&data).is_err());
}
#[test]
fn absent_tag_definitions_preserve_valid_tags_and_unresolved_assignments() {
    let mut data = database();
    word(&mut data, 8192 + 40 + 8, 999);
    let tags = MyTags::parse(&data).unwrap();
    assert_eq!(tags.values[&8].name, "Warm");
    assert!(tags.track_tags[&42].contains(&999));
    assert!(!tags.values.contains_key(&999));
    // A tag definition with a missing parent is still malformed.
    let mut data = database();
    let child = 4096 + 40 + definition(0, 7, "Mood").len();
    word(&mut data, child + 12, 999);
    assert!(MyTags::parse(&data).is_err());
}
#[test]
fn short_files_never_panic() {
    let data = database();
    for length in [0, 4, 100, 4095, 4096, 8192, 12287] {
        assert!(MyTags::parse(&data[..length]).is_err());
    }
}
