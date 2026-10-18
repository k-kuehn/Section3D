// Embeds the application icon and version info into section3d.exe.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("assets/section3d.rc", embed_resource::NONE).manifest_optional().ok();
    }
    println!("cargo:rerun-if-changed=assets/section3d.rc");
    println!("cargo:rerun-if-changed=assets/icon.ico");
}
