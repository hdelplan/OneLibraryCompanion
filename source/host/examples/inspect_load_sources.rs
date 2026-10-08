//! Read-only comparison of selected tracks in the two USB export databases.
#[path = "../src/onelibrary.rs"]
mod onelibrary;
use prolink_rekordbox::{AnlzFile, Library};
use serde_json::json;
use std::path::Path;

fn inspect(root: &Path, source: &str, library: &Library) {
    for track in library.tracks.values().filter(|t| {
        [892, 1162, 1863, 1921].contains(&t.id)
            || t.title.contains("Beau Mot Plage")
            || t.title.contains("Monologue")
            || t.title.contains("Soro te karaba")
            || t.title.contains("Amnésie")
            || t.title.contains("Danz Danz")
    }) {
        let paths = [
            Some(track.analyze_path.clone()),
            track.analyze_ext_path(),
            Some(
                Path::new(&track.analyze_path)
                    .with_extension("2EX")
                    .to_string_lossy()
                    .into_owned(),
            ),
        ];
        let analysis: Vec<_> = paths.into_iter().flatten().map(|path| {
            let result = std::fs::read(root.join(path.trim_start_matches('/')))
                .map_err(|e| e.to_string()).and_then(|bytes| {
                    let file = AnlzFile::parse(&bytes).map_err(|e| e.to_string())?;
                    Ok(json!({"bytes":bytes.len(),"tags":file.fourccs().iter().map(|tag| format!("{tag:?}")).collect::<Vec<_>>(),
                        "audioPath":file.path(),
                        "basic":file.cue_lists().map(|list| json!({"kind":list.list_type.0,"cues":list.cues.iter().map(|c|json!({"slot":c.hot_cue,"type":c.cue_type.0,"time":c.time,"end":c.loop_time,"status":format!("{:?}",c.status)})).collect::<Vec<_>>()})).collect::<Vec<_>>(),
                        "extended":file.extended_cue_lists().map(|list| json!({"kind":list.list_type.0,"cues":list.cues.iter().map(|c|json!({"slot":c.hot_cue,"type":c.cue_type.0,"time":c.time,"end":c.loop_time})).collect::<Vec<_>>()})).collect::<Vec<_>>() }))
                });
            json!({"path":path,"result":result})
        }).collect();
        println!(
            "{}",
            json!({"database":source,"id":track.id,"title":track.title,"artist":track.artist,"album":track.album,"comment":track.comment,
            "audio":track.file_path,"format":format!("{:?}",track.container),"rate":track.sample_rate,"depth":track.sample_depth,"duration":track.duration,"fileSize":track.file_size,"analysis":analysis})
        );
    }
}
fn main() -> Result<(), String> {
    let root = std::env::args()
        .nth(1)
        .ok_or("Usage: inspect_load_sources USB-root")?;
    let root = Path::new(&root);
    let (library, _, _) = onelibrary::read(&root.join(onelibrary::DATABASE))?;
    inspect(root, "OneLibrary", &library);
    let bytes =
        std::fs::read(root.join("PIONEER/rekordbox/export.pdb")).map_err(|e| e.to_string())?;
    let library = Library::parse(&bytes).map_err(|e| e.to_string())?;
    inspect(root, "PDB", &library);
    Ok(())
}
