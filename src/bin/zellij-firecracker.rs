#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn main() {
    if let Err(error) = zellij_server::firecracker::provider::main() {
        eprintln!("Error: {error:#}");
        std::process::exit(1);
    }
}

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
fn main() {
    eprintln!("zellij-firecracker is supported only on Linux x86_64");
    std::process::exit(1);
}
