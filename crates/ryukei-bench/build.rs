//! With the `occt` feature, builds the C++ shim in `src/occt/` and links
//! the OCCT toolkits it uses, from `OCCT_DIR` (default `/usr`: headers in
//! `include/opencascade`, libraries on the default search path or in
//! `lib`). Without it, does nothing: the default build has no C++.

fn main() {
    #[cfg(feature = "occt")]
    occt();
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
        "TKMesh",
    ] {
        println!("cargo:rustc-link-lib={lib}");
    }
    for f in ["src/occt.rs", "src/occt/shim.cc", "src/occt/shim.h"] {
        println!("cargo:rerun-if-changed={f}");
    }
    println!("cargo:rerun-if-env-changed=OCCT_DIR");
}
