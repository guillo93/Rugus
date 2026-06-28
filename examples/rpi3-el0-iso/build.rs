//! Pasa el linker script `link.ld` al enlazador y reconstruye si cambia.

fn main() {
    println!("cargo:rustc-link-search={}", env!("CARGO_MANIFEST_DIR"));
    println!("cargo:rerun-if-changed=link.ld");
    println!("cargo:rerun-if-changed=build.rs");
}
