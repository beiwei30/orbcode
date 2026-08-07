fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "connect_local",
            "connect_ssh",
            "protocol_send",
            "disconnect",
            "connection_status",
        ]),
    ))
    .expect("failed to build desktop host metadata");
}
