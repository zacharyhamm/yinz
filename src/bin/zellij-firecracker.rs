#[cfg(target_os = "linux")]
fn main() {
    if let Err(error) = zellij_server::firecracker::provider::main() {
        eprintln!("Error: {error:#}");
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("zellij-firecracker is supported only on Linux x86_64");
    std::process::exit(1);
}
