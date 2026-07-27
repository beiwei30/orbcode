#[cfg(any(target_os = "macos", target_os = "linux"))]
mod tracking {
    use super::super::*;
    use std::sync::RwLock;

    async fn cwd_tracking_context(label: &str) -> (ToolContext, Arc<RwLock<PathBuf>>) {
        let mut ctx = test_context(label).await;
        let shared_cwd = Arc::new(RwLock::new(ctx.cwd.clone()));
        let cwd_ref = shared_cwd.clone();
        ctx.on_cwd_change = Some(Arc::new(move |new_cwd: &std::path::Path| {
            *cwd_ref.write().unwrap() = new_cwd.to_path_buf();
        }));
        (ctx, shared_cwd)
    }

    fn paths_equivalent(a: &std::path::Path, b: &std::path::Path) -> bool {
        match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
            (Ok(ca), Ok(cb)) => ca == cb,
            _ => a == b,
        }
    }

    #[tokio::test]
    async fn consecutive_bash_calls_track_cwd() {
        let registry = ToolRegistry::foundation();
        let (ctx1, shared_cwd) = cwd_tracking_context("bash-cwd-consecutive").await;

        registry
            .invoke("bash", r#"{"command":"cd /tmp && pwd"}"#, &ctx1)
            .await
            .expect("first bash call");

        let new_cwd = shared_cwd.read().unwrap().clone();
        assert!(
            paths_equivalent(&new_cwd, std::path::Path::new("/tmp")),
            "shared cwd should be updated to /tmp, got: {}",
            new_cwd.display()
        );

        let mut ctx2 = ctx1.clone();
        ctx2.cwd = new_cwd;
        let result2 = registry
            .invoke("bash", r#"{"command":"pwd"}"#, &ctx2)
            .await
            .expect("second bash call");

        assert!(
            result2.output.contains("tmp"),
            "second bash pwd should report /tmp, got: {}",
            result2.output
        );
    }

    #[tokio::test]
    async fn bash_cwd_change_exposes_new_cwd_in_metadata() {
        let registry = ToolRegistry::foundation();
        let (ctx, _shared_cwd) = cwd_tracking_context("bash-cwd-metadata").await;

        let result = registry
            .invoke("bash", r#"{"command":"cd /tmp"}"#, &ctx)
            .await
            .expect("bash cd");

        let metadata = result.metadata.expect("should have metadata");
        let bash_meta = &metadata["bash"];
        let new_cwd = bash_meta["newCwd"]
            .as_str()
            .expect("newCwd should be present");
        assert!(
            new_cwd.contains("tmp"),
            "newCwd should reference /tmp, got: {new_cwd}"
        );
    }

    #[tokio::test]
    async fn bash_failed_cd_does_not_update_cwd() {
        let registry = ToolRegistry::foundation();
        let (ctx, shared_cwd) = cwd_tracking_context("bash-cwd-failed-cd").await;
        let original_cwd = ctx.cwd.clone();

        let _ = registry
            .invoke(
                "bash",
                r#"{"command":"cd /this_path_does_not_exist_orbcode 2>/dev/null"}"#,
                &ctx,
            )
            .await;

        let current = shared_cwd.read().unwrap().clone();
        assert_eq!(current, original_cwd, "cwd should not change when cd fails");
    }

    #[tokio::test]
    async fn bash_nonzero_exit_still_tracks_cwd_change() {
        let registry = ToolRegistry::foundation();
        let (ctx, shared_cwd) = cwd_tracking_context("bash-cwd-nonzero-exit").await;

        let result = registry
            .invoke("bash", r#"{"command":"cd /tmp; false"}"#, &ctx)
            .await;

        assert!(result.is_err(), "command should fail with non-zero exit");

        let new_cwd = shared_cwd.read().unwrap().clone();
        assert!(
            paths_equivalent(&new_cwd, std::path::Path::new("/tmp")),
            "cwd should be updated even with non-zero exit, got: {}",
            new_cwd.display()
        );
    }
}
