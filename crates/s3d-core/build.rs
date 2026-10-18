// Builds vendored Manifold plus the C shim in csrc/ as static libraries.
use std::path::PathBuf;

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let manifold_src = root.join("../../third_party/manifold");
    let dst = cmake::Config::new(root.join("csrc"))
        .define("MANIFOLD_SRC", manifold_src.to_str().unwrap().replace('\\', "/"))
        // Manifold is unusably slow in Debug; always build it optimised.
        .profile("Release")
        // The cmake crate's default flags carry no optimisation level.
        .define("CMAKE_CXX_FLAGS_RELEASE", "/O2 /Ob2 /DNDEBUG /MD /EHsc /nologo /W0")
        .build();
    println!("cargo:rustc-link-search=native={}", dst.join("lib").display());
    println!("cargo:rustc-link-lib=static=s3d_manifold");
    println!("cargo:rustc-link-lib=static=manifold");
    println!("cargo:rerun-if-changed=csrc");
}
