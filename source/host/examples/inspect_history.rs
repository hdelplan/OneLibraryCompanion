//! Read-only inspection of a Rekordbox export's set-history candidates.
fn main() {
    let path = std::env::args().nth(1).expect("export.pdb path");
    let lib = prolink_rekordbox::Library::parse(&std::fs::read(path).unwrap()).unwrap();
    println!(
        "Tracks: {}; history playlists: {}",
        lib.tracks.len(),
        lib.history.len()
    );
    for history in lib.history.values() {
        println!(
            "History {} {:?}: {} tracks",
            history.id,
            history.name,
            history.track_ids.len()
        );
    }
    for p in lib
        .playlists
        .values()
        .filter(|p| p.name.to_lowercase().contains("max4"))
    {
        println!(
            "Playlist {} {:?}: {} tracks",
            p.id,
            p.name,
            p.track_ids.len()
        );
        let mut keys = std::collections::BTreeMap::new();
        for id in &p.track_ids {
            if let Some(t) = lib.tracks.get(id) {
                *keys.entry(t.key.clone()).or_insert(0) += 1;
            }
        }
        println!("Keys: {keys:?}");
    }
}
