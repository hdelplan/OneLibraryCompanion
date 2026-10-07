// SPDX-License-Identifier: GPL-3.0-only
//! My Tag definitions and memberships from exportExt.pdb.
//! Layout derived from the local Prolink My Tags experiment; malformed rows fail
//! the extension as a whole rather than masquerading as untagged tracks.
//! References to absent tag definitions are retained for caller diagnostics:
//! real exports can retain relationships after a tag definition disappears.
use crate::{DeviceSqlString, Error, PageType, Pdb, Result};
use binrw::BinReaderExt;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Cursor,
};

/// Definitions and per-track assignments from one extension database.
#[derive(Debug, Clone, Default)]
pub struct MyTags {
    /// Category names keyed by exported identifier.
    pub categories: BTreeMap<u32, String>,
    /// Tag definitions keyed by exported identifier.
    pub values: BTreeMap<u32, MyTag>,
    /// Assigned tag identifiers for each track.
    pub track_tags: BTreeMap<u32, BTreeSet<u32>>,
}
/// One named tag within a category.
#[derive(Debug, Clone)]
pub struct MyTag {
    /// Exported tag identifier.
    pub id: u32,
    /// Parent category identifier.
    pub category_id: u32,
    /// Exported order within its category.
    pub index: u32,
    /// Display name.
    pub name: String,
}
fn word(row: &[u8], at: usize) -> Result<u32> {
    let bytes = row
        .get(at..at + 4)
        .ok_or_else(|| Error::malformed(at as u64, "short My Tag row"))?;
    Ok(u32::from_le_bytes(
        bytes
            .try_into()
            .map_err(|_| Error::malformed(at as u64, "short word"))?,
    ))
}
impl MyTags {
    /// Parse an extension, rejecting malformed definitions or missing categories.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let pdb = Pdb::new(data)?;
        if pdb.table(PageType(3)).is_none() || pdb.table(PageType(4)).is_none() {
            return Err(Error::malformed(0, "My Tag tables are absent"));
        }
        if data.len() % pdb.page_size() as usize != 0 {
            return Err(Error::malformed(0, "incomplete extension database page"));
        }
        for kind in [3, 4] {
            let table = pdb
                .table(PageType(kind))
                .ok_or_else(|| Error::malformed(0, "missing My Tag table"))?;
            for page in [table.first_page, table.last_page] {
                if page != 0
                    && (u64::from(page) + 1) * u64::from(pdb.page_size()) > data.len() as u64
                {
                    return Err(Error::malformed(0, "missing My Tag table page"));
                }
            }
        }
        let mut tags = Self::default();
        for kind in [3, 4] {
            for (page, header) in pdb.pages(PageType(kind)) {
                let mut offsets = pdb.row_offsets(page, &header);
                offsets.sort_unstable();
                offsets.dedup();
                // Bound reads to the row heap, excluding the footer index.
                let page_start = u64::from(page) * u64::from(pdb.page_size());
                let footer = u64::from(header.num_rows()).div_ceil(16) * 36;
                let end = (page_start + u64::from(pdb.page_size()))
                    .checked_sub(footer)
                    .filter(|end| *end >= page_start + 40)
                    .ok_or_else(|| Error::malformed(page_start, "invalid My Tag page index"))?;
                for (i, &start) in offsets.iter().enumerate() {
                    let stop = offsets.get(i + 1).copied().unwrap_or(end).min(end);
                    let row = data
                        .get(start as usize..stop as usize)
                        .ok_or_else(|| Error::malformed(start, "invalid My Tag row bounds"))?;
                    tags.read_row(kind, row)?;
                }
            }
        }
        if tags
            .values
            .values()
            .any(|tag| !tags.categories.contains_key(&tag.category_id))
        {
            return Err(Error::malformed(0, "unresolved My Tag relationship"));
        }
        Ok(tags)
    }
    fn read_row(&mut self, kind: u32, row: &[u8]) -> Result<()> {
        if kind == 3 {
            let category_id = word(row, 0x0c)?;
            let index = word(row, 0x10)?;
            let id = word(row, 0x14)?;
            let offset = usize::from(
                *row.get(0x1d)
                    .ok_or_else(|| Error::malformed(0, "short tag definition"))?,
            );
            if offset < 0x1e {
                return Err(Error::malformed(0, "invalid tag name offset"));
            }
            let bytes = row
                .get(offset..)
                .ok_or_else(|| Error::malformed(0, "tag name outside row"))?;
            let name = Cursor::new(bytes).read_le::<DeviceSqlString>()?.text;
            if category_id == 0 {
                self.categories.insert(id, name);
            } else {
                self.values.insert(
                    id,
                    MyTag {
                        id,
                        category_id,
                        index,
                        name,
                    },
                );
            }
        } else if word(row, 0)? == 0 && word(row, 12)? == 3 {
            self.track_tags
                .entry(word(row, 4)?)
                .or_default()
                .insert(word(row, 8)?);
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn definitions_and_memberships() {
        let mut tags = MyTags::default();
        let mut row = vec![0; 34];
        row[0x14..0x18].copy_from_slice(&7u32.to_le_bytes());
        row[0x1d] = 30;
        row[30..].copy_from_slice(&[9, b'T', b'a', b'g']);
        tags.read_row(3, &row).unwrap();
        assert_eq!(tags.categories[&7], "Tag");
        row[0x0c..0x10].copy_from_slice(&7u32.to_le_bytes());
        row[0x14..0x18].copy_from_slice(&8u32.to_le_bytes());
        tags.read_row(3, &row).unwrap();
        let relation: Vec<u8> = [0u32, 99, 8, 3]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect();
        tags.read_row(4, &relation).unwrap();
        assert_eq!(tags.values[&8].category_id, 7);
        assert!(tags.track_tags[&99].contains(&8));
        for length in 0..30 {
            assert!(tags.read_row(3, &row[..length]).is_err());
        }
        row[0x1d] = 255;
        assert!(tags.read_row(3, &row).is_err());
    }
}
