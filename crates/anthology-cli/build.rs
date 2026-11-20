//! Records the target triple for `anthology --version` (design §6.1).
fn main() {
    let target = std::env::var("TARGET").expect("cargo sets TARGET for build scripts");
    println!("cargo:rustc-env=ANTHOLOGY_TARGET={target}");
    println!("cargo:rerun-if-changed=build.rs");
}
