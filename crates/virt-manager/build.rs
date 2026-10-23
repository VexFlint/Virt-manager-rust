//! Bundles upstream's icons into a GResource, and compiles upstream's
//! GSettings schema into `$OUT_DIR/schemas` so a dev build (`cargo run`)
//! has settings without anything installed system-wide — see
//! `src/config.rs` for the lookup order.

use std::process::Command;

fn main() {
    glib_build_tools::compile_resources(
        &["data"],
        "data/resources.gresource.xml",
        "virt-manager.gresource",
    );

    println!("cargo:rerun-if-changed=data/org.virt-manager.virt-manager.gschema.xml");
    let out_dir = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    let schema_dir = format!("{out_dir}/schemas");
    std::fs::create_dir_all(&schema_dir).expect("create schema dir");
    std::fs::copy(
        "data/org.virt-manager.virt-manager.gschema.xml",
        format!("{schema_dir}/org.virt-manager.virt-manager.gschema.xml"),
    )
    .expect("copy gschema");
    let status = Command::new("glib-compile-schemas")
        .arg(&schema_dir)
        .status()
        .expect("glib-compile-schemas (libglib2.0-bin) must be installed");
    assert!(status.success(), "glib-compile-schemas failed");
}
