fn main() {
    println!("cargo:rerun-if-changed=../../packaging/icons/icon.ico");
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        let mut resources = winres::WindowsResource::new();
        let icon = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../packaging/icons/icon.ico")
            .canonicalize()
            .expect("failed to resolve Windows icon path");
        resources.set_icon(icon.to_string_lossy().as_ref());
        resources
            .compile()
            .expect("failed to embed Windows resources");
    }
}
