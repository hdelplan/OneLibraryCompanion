//! Read-only OneLibrary adapter. Schema/key interoperability reference:
//! chrisle/onelibrary-connect (MIT); attribution in THIRD_PARTY_NOTICES.md.
use prolink_rekordbox::{
    Container, Library, Playlist, Track,
    mytags::{MyTag, MyTags},
};
use rusqlite::{Connection, OpenFlags, Row};
use std::{collections::BTreeMap, path::Path};
const KEY: &str = "r8gddnr4k847830ar6cqzbkk0el6qytmb3trbbx805jm74vez64i5o8fnrqryqls";
pub const DATABASE: &str = "PIONEER/rekordbox/exportLibrary.db";
const LIMIT: u64 = 64 * 1024 * 1024;
fn text(row: &Row<'_>, column: &str) -> String {
    row.get::<_, Option<String>>(column)
        .ok()
        .flatten()
        .unwrap_or_default()
}
fn number(row: &Row<'_>, column: &str) -> u32 {
    row.get::<_, Option<u32>>(column)
        .ok()
        .flatten()
        .unwrap_or_default()
}
fn table(db: &Connection, name: &str) -> bool {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [name],
        |r| r.get::<_, bool>(0),
    )
    .unwrap_or(false)
}
fn names(
    db: &Connection,
    name: &str,
    id: &str,
    value: &str,
) -> Result<BTreeMap<u32, String>, String> {
    if !table(db, name) {
        return Ok(BTreeMap::new());
    }
    // Identifiers are fixed internal schema names, never user inputs.
    let mut stmt = db
        .prepare(&format!("SELECT {id}, {value} FROM {name}"))
        .map_err(|e| e.to_string())?;
    stmt.query_map([], |r| {
        Ok((
            r.get(0)?,
            r.get::<_, Option<String>>(1)?.unwrap_or_default(),
        ))
    })
    .map_err(|e| e.to_string())?
    .collect::<Result<_, _>>()
    .map_err(|e| e.to_string())
}
pub fn read(path: &Path) -> Result<(Library, Option<MyTags>, String), String> {
    let metadata = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() < 4096 || metadata.len() > LIMIT {
        return Err("OneLibrary database must be a regular file between 4 KiB and 64 MiB".into());
    }
    let wal = path.with_file_name("exportLibrary.db-wal");
    if std::fs::metadata(wal).is_ok_and(|m| m.len() > 0) {
        return Err("OneLibrary is being updated; safely finish the export and retry".into());
    }
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| e.to_string())?;
    db.busy_timeout(std::time::Duration::from_secs(2))
        .map_err(|e| e.to_string())?;
    db.pragma_update(None, "key", KEY)
        .map_err(|e| e.to_string())?;
    db.pragma_update(None, "query_only", true)
        .map_err(|e| e.to_string())?;
    db.pragma_update(None, "trusted_schema", false)
        .map_err(|e| e.to_string())?;
    // Force decryption and verify a library schema, not merely a matching filename.
    for query in [
        "SELECT deviceName,dbVersion FROM property LIMIT 1",
        "SELECT content_id,title,path FROM content LIMIT 1",
        "SELECT playlist_id,name FROM playlist LIMIT 1",
    ] {
        db.prepare(query)
            .and_then(|mut s| s.exists([]))
            .map_err(|e| format!("Invalid or unsupported OneLibrary database: {e}"))?;
    }
    let integrity: String = db
        .query_row("PRAGMA quick_check(1)", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if integrity != "ok" {
        return Err("OneLibrary database integrity check failed".into());
    }
    let mut library = Library {
        artists: names(&db, "artist", "artist_id", "name")?,
        albums: names(&db, "album", "album_id", "name")?,
        genres: names(&db, "genre", "genre_id", "name")?,
        keys: names(&db, "key", "key_id", "name")?,
        labels: names(&db, "label", "label_id", "name")?,
        artwork: names(&db, "image", "image_id", "path")?,
        ..Library::default()
    };
    let colors = names(&db, "color", "color_id", "name")?;
    library.colors = colors
        .iter()
        .filter_map(|(id, n)| u8::try_from(*id).ok().map(|id| (id, n.clone())))
        .collect();
    let lookup =
        |values: &BTreeMap<u32, String>, id: u32| values.get(&id).cloned().unwrap_or_default();
    let count: u32 = db
        .query_row("SELECT COUNT(*) FROM content", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if count > 200_000 {
        return Err("OneLibrary exceeds the 200,000-track limit".into());
    }
    let mut statement = db
        .prepare("SELECT * FROM content")
        .map_err(|e| e.to_string())?;
    let mut rows = statement.query([]).map_err(|e| e.to_string())?;
    while let Some(r) = rows.next().map_err(|e| e.to_string())? {
        let id: u32 = r.get("content_id").map_err(|e| e.to_string())?;
        if id == 0 || library.tracks.contains_key(&id) {
            return Err("Invalid or duplicate OneLibrary track ID".into());
        }
        let artist_id = number(r, "artist_id_artist");
        let album_id = number(r, "album_id");
        let genre_id = number(r, "genre_id");
        let key_id = number(r, "key_id");
        let label_id = number(r, "label_id");
        let artwork_id = number(r, "image_id");
        let color_id = number(r, "color_id");
        let file_path = text(r, "path");
        let extension = Path::new(&file_path)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let container = match extension.as_str() {
            "mp3" => Container::MP3,
            "wav" => Container::WAV,
            "aif" | "aiff" => Container::AIFF,
            "flac" => Container::FLAC,
            "aac" => Container::AAC,
            _ => Container(0),
        };
        let track = Track {
            id,
            title: text(r, "title"),
            artist_id,
            album_id,
            genre_id,
            key_id,
            label_id,
            artwork_id,
            color_id: u8::try_from(color_id).unwrap_or(0),
            artist: lookup(&library.artists, artist_id),
            album: lookup(&library.albums, album_id),
            genre: lookup(&library.genres, genre_id),
            key: lookup(&library.keys, key_id),
            label: lookup(&library.labels, label_id),
            color: lookup(&colors, color_id),
            artwork_path: lookup(&library.artwork, artwork_id),
            remixer: lookup(&library.artists, number(r, "artist_id_remixer")),
            composer: lookup(&library.artists, number(r, "artist_id_composer")),
            original_artist: lookup(&library.artists, number(r, "artist_id_originalArtist")),
            tempo: number(r, "bpmx100"),
            duration: number(r, "length").min(u16::MAX as u32) as u16,
            rating: number(r, "rating").min(5) as u8,
            file_path,
            filename: text(r, "fileName"),
            analyze_path: text(r, "analysisDataFilePath"),
            comment: text(r, "djComment"),
            date_added: text(r, "dateAdded"),
            release_date: text(r, "releaseDate"),
            mix_name: text(r, "subtitle"),
            isrc: text(r, "isrc"),
            bitrate: number(r, "bitrate"),
            sample_rate: number(r, "samplingRate"),
            sample_depth: number(r, "bitDepth").min(u16::MAX as u32) as u16,
            file_size: number(r, "fileSize"),
            track_number: number(r, "trackNo"),
            disc_number: number(r, "discNo").min(u16::MAX as u32) as u16,
            year: number(r, "releaseYear").min(u16::MAX as u32) as u16,
            play_count: number(r, "djPlayCount").min(u16::MAX as u32) as u16,
            container,
            ..Track::default()
        };
        library.tracks.insert(id, track);
    }
    let mut statement = db
        .prepare("SELECT * FROM playlist")
        .map_err(|e| e.to_string())?;
    let mut rows = statement.query([]).map_err(|e| e.to_string())?;
    while let Some(r) = rows.next().map_err(|e| e.to_string())? {
        let id = r.get("playlist_id").map_err(|e| e.to_string())?;
        library.playlists.insert(
            id,
            Playlist {
                id,
                name: text(r, "name"),
                parent_id: number(r, "playlist_id_parent"),
                is_folder: number(r, "attribute") == 1,
                sort_order: number(r, "sequenceNo"),
                ..Playlist::default()
            },
        );
    }
    if table(&db, "playlist_content") {
        let mut statement = db
            .prepare("SELECT playlist_id,content_id FROM playlist_content ORDER BY sequenceNo")
            .map_err(|e| e.to_string())?;
        let entries = statement
            .query_map([], |r| Ok((r.get::<_, u32>(0)?, r.get::<_, u32>(1)?)))
            .map_err(|e| e.to_string())?;
        for entry in entries {
            let (list, track) = entry.map_err(|e| e.to_string())?;
            if library.tracks.contains_key(&track)
                && let Some(p) = library.playlists.get_mut(&list)
            {
                p.track_ids.push(track);
            }
        }
    }
    let mut children: Vec<_> = library
        .playlists
        .values()
        .map(|p| (p.parent_id, p.sort_order, p.id))
        .collect();
    children.sort();
    for (parent, _, id) in &children {
        if let Some(p) = library.playlists.get_mut(parent) {
            p.children.push(*id);
        }
    }
    let mut tags = MyTags::default();
    let tags_available = table(&db, "myTag") && table(&db, "myTag_content");
    if tags_available {
        let mut statement = db
            .prepare("SELECT * FROM myTag")
            .map_err(|e| e.to_string())?;
        let mut rows = statement.query([]).map_err(|e| e.to_string())?;
        while let Some(r) = rows.next().map_err(|e| e.to_string())? {
            let id = r.get("myTag_id").map_err(|e| e.to_string())?;
            let name = text(r, "name");
            if number(r, "attribute") == 1 {
                tags.categories.insert(id, name);
            } else {
                tags.values.insert(
                    id,
                    MyTag {
                        id,
                        name,
                        category_id: number(r, "myTag_id_parent"),
                        index: number(r, "sequenceNo"),
                    },
                );
            }
        }
        let mut statement = db
            .prepare("SELECT content_id,myTag_id FROM myTag_content")
            .map_err(|e| e.to_string())?;
        for row in statement
            .query_map([], |r| Ok((r.get::<_, u32>(0)?, r.get::<_, u32>(1)?)))
            .map_err(|e| e.to_string())?
        {
            let (track, tag) = row.map_err(|e| e.to_string())?;
            tags.track_tags.entry(track).or_default().insert(tag);
        }
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let after = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if after.len() != metadata.len() || after.modified().ok() != metadata.modified().ok() {
        return Err("OneLibrary changed while reading; retry after the export finishes".into());
    }
    library.digest = prolink_rekordbox::stable_digest(&bytes);
    let fingerprint = format!("onelibrary:{}", library.digest);
    Ok((library, tags_available.then_some(tags), fingerprint))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    pub struct Fixture(pub std::path::PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    pub fn fixture() -> Fixture {
        let root = std::env::temp_dir().join(format!(
            "pc-onelibrary-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("PIONEER/rekordbox")).unwrap();
        let db = Connection::open(root.join(DATABASE)).unwrap();
        let version: String = db
            .query_row("PRAGMA cipher_version", [], |r| r.get(0))
            .unwrap();
        assert!(!version.is_empty());
        db.pragma_update(None, "key", KEY).unwrap();
        db.execute_batch("CREATE TABLE property(deviceName TEXT,dbVersion TEXT); INSERT INTO property VALUES('Test USB','1');
            CREATE TABLE content(content_id INTEGER,title TEXT,path TEXT,bpmx100 INTEGER,length INTEGER,artist_id_artist INTEGER,genre_id INTEGER,rating INTEGER,samplingRate INTEGER,bitDepth INTEGER);
            INSERT INTO content VALUES(1,'Track One','/Contents/one.wav',12500,240,10,20,4,44100,24);
            INSERT INTO content VALUES(2,'Track Two','/Contents/two.flac',12800,180,10,20,3,48000,16);
            CREATE TABLE artist(artist_id INTEGER,name TEXT); INSERT INTO artist VALUES(10,'Test Artist');
            CREATE TABLE genre(genre_id INTEGER,name TEXT); INSERT INTO genre VALUES(20,'House');
            CREATE TABLE playlist(playlist_id INTEGER,name TEXT,attribute INTEGER,playlist_id_parent INTEGER,sequenceNo INTEGER);
            INSERT INTO playlist VALUES(5,'Test playlist',0,NULL,1);
            CREATE TABLE playlist_content(playlist_id INTEGER,content_id INTEGER,sequenceNo INTEGER);
            INSERT INTO playlist_content VALUES(5,1,2),(5,2,1);
            CREATE TABLE myTag(myTag_id INTEGER,name TEXT,attribute INTEGER,myTag_id_parent INTEGER,sequenceNo INTEGER);
            INSERT INTO myTag VALUES(10,'Mood',1,NULL,0),(11,'Warm',0,10,1);
            CREATE TABLE myTag_content(content_id INTEGER,myTag_id INTEGER); INSERT INTO myTag_content VALUES(1,11);").unwrap();
        Fixture(root)
    }
    #[test]
    fn decrypts_read_only_and_preserves_metadata_playlists_and_tags() {
        let root = fixture();
        let path = root.0.join(DATABASE);
        let before = std::fs::read(&path).unwrap();
        assert!(!before.starts_with(b"SQLite format 3"));
        let (lib, tags, _) = read(&path).unwrap();
        assert_eq!(lib.tracks.len(), 2);
        assert_eq!(lib.tracks[&1].artist, "Test Artist");
        assert_eq!(lib.tracks[&1].genre, "House");
        assert_eq!(lib.tracks[&1].bpm(), 125.0);
        assert_eq!(lib.tracks[&1].sample_depth, 24);
        assert_eq!(lib.playlists[&5].track_ids, vec![2, 1]);
        let tags = tags.unwrap();
        assert_eq!(tags.categories[&10], "Mood");
        assert!(tags.track_tags[&1].contains(&11));
        assert_eq!(std::fs::read(path).unwrap(), before);
    }
    #[test]
    fn rejects_fake_database_and_pending_export() {
        let root = fixture();
        let path = root.0.join(DATABASE);
        std::fs::write(path.with_file_name("exportLibrary.db-wal"), [1, 2, 3]).unwrap();
        assert!(read(&path).unwrap_err().contains("updated"));
        std::fs::remove_file(path.with_file_name("exportLibrary.db-wal")).unwrap();
        std::fs::write(&path, vec![0; 8192]).unwrap();
        assert!(read(&path).is_err());
    }
}
