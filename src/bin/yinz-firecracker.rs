#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn main() {
    if let Err(error) = yinz_firecracker_provider::main() {
        eprintln!("Error: {error:#}");
        std::process::exit(1);
    }
}

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
fn main() {
    eprintln!("yinz-firecracker is supported only on Linux x86_64");
    std::process::exit(1);
}
