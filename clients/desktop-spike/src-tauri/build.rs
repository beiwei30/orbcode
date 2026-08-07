fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(&["run_initialize_probe"])),
    )
    .expect("failed to build desktop spike metadata");
}
