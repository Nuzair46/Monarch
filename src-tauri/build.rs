fn main() {
    if std::env::var_os("MONARCH_SKIP_TAURI_BUILD").is_some() {
        println!("cargo:warning=Skipping tauri-build because MONARCH_SKIP_TAURI_BUILD is set");
        return;
    }
    tauri_build::build()
}
