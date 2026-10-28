//! lib3mf, the 3MF Consortium's reference library (BSD-2-Clause), through
//! a small C++ shim, as a reference for the 3MF files ryucad writes: the
//! file read in strict mode, its warnings, unit, metadata and objects, and
//! the volume its build encloses with every component placed. Only built
//! with the `formats` feature, against lib3mf 2.5 (`LIB3MF_DIR`, default
//! `/usr/local`; the devcontainer builds it).

#[cxx::bridge(namespace = "ryukei_lib3mf")]
mod ffi {
    /// What lib3mf read: its warnings (none for a sound file), the
    /// model's unit, each metadata entry as `[namespace:]name=value`, each
    /// object as `name|partnumber`, the build's item count, and the
    /// triangles and volume the build places, in the file's unit.
    #[derive(Debug, Default)]
    struct Read3mf {
        warnings: Vec<String>,
        unit: String,
        metadata: Vec<String>,
        objects: Vec<String>,
        items: u32,
        triangles: u64,
        volume: f64,
    }

    unsafe extern "C++" {
        include!("ryukei-bench/src/lib3mf/shim.h");

        fn read_3mf(path: &str) -> Result<Read3mf>;
    }
}

pub use ffi::Read3mf;

/// The 3MF file at `path`, as lib3mf reads it; its failure as text.
pub fn read(path: &std::path::Path) -> Result<Read3mf, String> {
    ffi::read_3mf(&path.to_string_lossy()).map_err(|e| e.to_string())
}
