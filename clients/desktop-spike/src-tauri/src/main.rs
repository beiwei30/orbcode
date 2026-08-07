use orbcode_desktop_spike::{
    INTERNAL_CHILD_FLAG, ProbeChild, configure_builder, navigation_guard, run_protocol_test_child,
};

fn main() {
    if std::env::args().any(|argument| argument == INTERNAL_CHILD_FLAG) {
        if let Err(error) = run_protocol_test_child() {
            eprintln!("desktop spike protocol child failed: {error}");
            std::process::exit(2);
        }
        return;
    }

    let child = ProbeChild::current_executable().unwrap_or_else(|error| {
        eprintln!("desktop spike startup failed: {error}");
        std::process::exit(1);
    });
    configure_builder(tauri::Builder::default(), child)
        .plugin(navigation_guard())
        .run(tauri::generate_context!())
        .expect("desktop spike runtime failed");
}
