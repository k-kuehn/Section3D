// Builds vendored Manifold plus the C shim in csrc/ as static libraries.
use std::path::PathBuf;

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let manifold_src = root.join("../../third_party/manifold");
    // Match the Rust side's C runtime (static when +crt-static is set).
    let static_crt = std::env::var("CARGO_CFG_TARGET_FEATURE").unwrap_or_default().split(',').any(|f| f == "crt-static");
    let (rt, rt_flag) = if static_crt { ("MultiThreaded", "/MT") } else { ("MultiThreadedDLL", "/MD") };
    let dst = cmake::Config::new(root.join("csrc"))
        .define("MANIFOLD_SRC", manifold_src.to_str().unwrap().replace('\\', "/"))
        .define("CMAKE_MSVC_RUNTIME_LIBRARY", rt)
        .static_crt(static_crt)
        // Manifold is unusably slow in Debug; always build it optimised.
        .profile("Release")
        // The cmake crate's default flags carry no optimisation level.
        .define("CMAKE_CXX_FLAGS_RELEASE", format!("/O2 /Ob2 /DNDEBUG {rt_flag} /EHsc /nologo /W0"))
        .build();
    println!("cargo:rustc-link-search=native={}", dst.join("lib").display());
    println!("cargo:rustc-link-lib=static=s3d_manifold");
    println!("cargo:rustc-link-lib=static=manifold");
    println!("cargo:rerun-if-changed=csrc");
}
