use embed_manifest::manifest::{ActiveCodePage, DpiAwareness};
use embed_manifest::{embed_manifest, new_manifest};

fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        embed_manifest(new_manifest("hyprwin").dpi_awareness(DpiAwareness::PerMonitorV2).active_code_page(ActiveCodePage::Utf8))
            .expect("unable to embed manifest");
        embed_resource::compile("app.rc", embed_resource::NONE).manifest_optional().unwrap();
    }
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=build.rs");
}
