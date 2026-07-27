//! Brand audit — enforced through `cargo test`, not just `scripts/check.sh`.
//!
//! The real logic lives in `scripts/audit-brand.sh` so it can also run as a
//! standalone pre-push check. This wrapper exists so that a `cargo test
//! --workspace` (or CI running only tests) still fails on a leak, without anyone
//! having to remember the script.
//!
//! It enforces both directions:
//!   - the pre-rename product, crate-path and env-prefix spellings must not
//!     reappear (this file cannot spell them out without tripping its own
//!     check; the script's header lists them and the paths exempt from the scan)
//!   - the TypeScript-CLI compatibility names (`CLAUDE_CONFIG_DIR`,
//!     `claude_code_version`, `x-claude-code-session-id`, ...) must still be
//!     present in source, so an over-eager rename cannot silently break
//!     on-disk/wire compatibility

use std::path::Path;
use std::process::Command;

#[test]
fn brand_audit_script_passes() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root");
    let script = repo_root.join("scripts/audit-brand.sh");

    // Skip rather than fail where the script cannot run at all (e.g. a package
    // vendored without scripts/, or a non-bash environment): this test guards
    // against regressions, and a missing harness is not a regression.
    if !script.is_file() {
        eprintln!("skipping: {} not present", script.display());
        return;
    }

    let output = match Command::new("bash")
        .arg(&script)
        .current_dir(repo_root)
        .output()
    {
        Ok(output) => output,
        Err(error) => {
            eprintln!("skipping: could not run bash: {error}");
            return;
        }
    };

    assert!(
        output.status.success(),
        "scripts/audit-brand.sh failed.\n--- stdout ---\n{}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
