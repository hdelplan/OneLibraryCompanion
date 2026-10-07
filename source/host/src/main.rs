#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    pioneer_companion_host::run_desktop().await
}
