//! Build script: copia `memory.x` al `OUT_DIR` y lo añade al search path
//! del linker para que el `INCLUDE memory.x` de `link.x` (cortex-m-rt) lo
//! encuentre.
//!
//! Este es el setup canónico para binarios cortex-m-rt; sin esto, el linker
//! reporta `cannot find linker script memory.x`.

use std::env;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR not set"));
    File::create(out.join("memory.x"))
        .expect("failed to create memory.x in OUT_DIR")
        .write_all(include_bytes!("memory.x"))
        .expect("failed to write memory.x");

    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rerun-if-changed=build.rs");
}
