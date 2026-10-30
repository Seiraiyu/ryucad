//! With the `occt` feature, builds the C++ shim in `src/occt/` and links
//! the OCCT toolkits it uses, from `OCCT_DIR` (default `/usr`: headers in
//! `include/opencascade`, libraries on the default search path or in
//! `lib`). With the `smlib` feature, builds the shim in `src/smlib/` and
//! links `libsmlib`, from `SMLIB_DIR` (default `/usr/local`: headers in
//! `include/smlib`, the library in `lib`). With the `formats` feature,
//! builds the shim in `src/lib3mf/` and links `lib3mf`, from `LIB3MF_DIR`
//! (default `/usr/local`: headers in `include/lib3mf`, the library in
//! `lib`). Without them, does nothing: the default build has no C++.

fn main() {
    #[cfg(feature = "occt")]
    occt();
    #[cfg(feature = "smlib")]
    smlib();
    #[cfg(feature = "formats")]
    lib3mf();
}

#[cfg(feature = "occt")]
fn occt() {
    let dir = std::env::var("OCCT_DIR").unwrap_or_else(|_| "/usr".into());
    let dir = std::path::Path::new(&dir);
    cxx_build::bridge("src/occt.rs")
        .file("src/occt/shim.cc")
        .include(dir.join("include/opencascade"))
        .std("c++17")
        .warnings(false)
        .compile("ryukei_occt");
    println!(
        "cargo:rustc-link-search=native={}",
        dir.join("lib").display()
    );
    for lib in [
        "TKernel",
        "TKMath",
        "TKG2d",
        "TKG3d",
        "TKGeomBase",
        "TKBRep",
        "TKGeomAlgo",
        "TKTopAlgo",
        "TKPrim",
        "TKBO",
        "TKMesh",
        "TKFillet",
        "TKShHealing",
        "TKXSBase",
        "TKDE",
        "TKDESTEP",
        "TKDESTL",
    ] {
        println!("cargo:rustc-link-lib={lib}");
    }
    for f in ["src/occt.rs", "src/occt/shim.cc", "src/occt/shim.h"] {
        println!("cargo:rerun-if-changed={f}");
    }
    println!("cargo:rerun-if-env-changed=OCCT_DIR");
}

#[cfg(feature = "smlib")]
fn smlib() {
    let dir = std::env::var("SMLIB_DIR").unwrap_or_else(|_| "/usr/local".into());
    let dir = std::path::Path::new(&dir);
    cxx_build::bridge("src/smlib.rs")
        .file("src/smlib/shim.cc")
        .include(dir.join("include/smlib"))
        .std("c++17")
        .warnings(false)
        .compile("ryukei_smlib");
    println!(
        "cargo:rustc-link-search=native={}",
        dir.join("lib").display()
    );
    for lib in ["smlib", "tbb"] {
        println!("cargo:rustc-link-lib={lib}");
    }
    for f in ["src/smlib.rs", "src/smlib/shim.cc", "src/smlib/shim.h"] {
        println!("cargo:rerun-if-changed={f}");
    }
    println!("cargo:rerun-if-env-changed=SMLIB_DIR");
}

#[cfg(feature = "formats")]
fn lib3mf() {
    let dir = std::env::var("LIB3MF_DIR").unwrap_or_else(|_| "/usr/local".into());
    let dir = std::path::Path::new(&dir);
    cxx_build::bridge("src/lib3mf.rs")
        .file("src/lib3mf/shim.cc")
        .include(dir.join("include/lib3mf"))
        .std("c++17")
        .warnings(false)
        .compile("ryukei_lib3mf");
    println!(
        "cargo:rustc-link-search=native={}",
        dir.join("lib").display()
    );
    println!("cargo:rustc-link-lib=3mf");
    for f in ["src/lib3mf.rs", "src/lib3mf/shim.cc", "src/lib3mf/shim.h"] {
        println!("cargo:rerun-if-changed={f}");
    }
    println!("cargo:rerun-if-env-changed=LIB3MF_DIR");
}
