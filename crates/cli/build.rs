fn main() {
    // Cargo always sets `TARGET` for build scripts to the triple being
    // compiled for (the host triple for a native build, the `--target`
    // triple for a cross build). Exporting it as a compile-time env var lets
    // `ourshell update` (crates/cli/src/update.rs) pick the exact same
    // release asset name the GitHub Actions build matrix in
    // `.github/workflows/release.yml` produces for this binary, without
    // re-deriving it from `std::env::consts::{OS, ARCH}` (which can't tell
    // MSVC from GNU, or distinguish libc flavors).
    let target = std::env::var("TARGET").expect("cargo did not set TARGET");
    println!("cargo:rustc-env=OURSHELL_TARGET={target}");
    println!("cargo:rerun-if-env-changed=TARGET");
}
