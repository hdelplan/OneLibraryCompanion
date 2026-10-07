//! Import a local export without starting a CDJ monitor or modifying the USB.
use pioneer_companion_host::set_history::Store;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let database = args
        .next()
        .ok_or("Usage: import_set_history export.pdb data-directory")?;
    let dir = args.next().ok_or("Missing data directory")?;
    let library = prolink_rekordbox::Library::parse(&std::fs::read(database)?)?;
    let mut store = Store::open(dir.into());
    let added = store.import(&library)?;
    let view = store.snapshot();
    println!("{}", view["importNote"]);
    println!(
        "{} new track entries. {} total sets.",
        added.len(),
        view["sets"].as_array().unwrap().len()
    );
    Ok(())
}
