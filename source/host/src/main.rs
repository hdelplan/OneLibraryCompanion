#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "macos")]
    if std::env::args().any(|arg| arg == "--check-local-usb-helper") {
        let socket = pioneer_companion_host::mac_networking::acquire()?;
        println!(
            "Local USB networking ready: {} (user {})",
            socket.local_addr()?,
            unsafe { libc::geteuid() }
        );
        return Ok(());
    }
    pioneer_companion_host::run_desktop().await
}
