fn main() {
    tracing::info!("desktop application bootstrap");

    let app_name = app_core::app_name();
    println!("{app_name} desktop skeleton");
}
