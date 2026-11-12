use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=../../packaging/icons/icon.ico");
    copy_duckdb_dll();
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        let mut resources = winres::WindowsResource::new();
        let icon = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../packaging/icons/icon.ico")
            .canonicalize()
            .expect("failed to resolve Windows icon path");
        resources.set_icon(icon.to_string_lossy().as_ref());
        resources
            .compile()
            .expect("failed to embed Windows resources");
    }
}

/// 把打包目录里的 duckdb.dll 复制到最终 exe 所在目录，
/// 否则运行时找不到动态库（STATUS_DLL_NOT_FOUND）。
fn copy_duckdb_dll() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_none() {
        return; // 仅 Windows 需要。Linux/macOS 走 LD_LIBRARY_PATH 或 rpath。
    }
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src = manifest.join("../../packaging/duckdb-download/local/1.5.4/duckdb.dll");
    // 库文件变化时强制重跑本脚本
    println!("cargo:rerun-if-changed={}", src.display());

    if !src.exists() {
        println!(
            "cargo:warning=duckdb.dll not found at {}. Put it there before running.",
            src.display()
        );
        return;
    }

    // OUT_DIR = target/{profile}/build/{crate}-{hash}/out
    // ancestors(3) 即 target/{profile}，也就是 orvyn.exe 所在目录。
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let profile_dir = out_dir
        .ancestors()
        .nth(3)
        .expect("OUT_DIR does not have the expected target/{profile}/build/... layout");
    let dest = profile_dir.join("duckdb.dll");

    let need_copy = std::fs::read(&src).ok() != std::fs::read(&dest).ok();
    if need_copy {
        std::fs::copy(&src, &dest).expect("failed to copy duckdb.dll next to the exe");
        println!("cargo:warning=copied duckdb.dll -> {}", dest.display());
    } else {
        println!("cargo:info=duckdb.dll is up to date at {}", dest.display());
    }
}
