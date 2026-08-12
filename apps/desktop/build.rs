fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        let mut resources = winres::WindowsResource::new();
        resources.set_icon("../../packaging/icons/icon.ico");
        resources
            .compile()
            .expect("failed to embed Windows resources");
    }
}
