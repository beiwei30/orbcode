use std::process::Command;

fn main() {
    let sha = git_short_sha().unwrap_or_else(|| "unknown".to_string());
    let dirty = if git_dirty() { "+dirty" } else { "" };
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_string());
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "unknown".to_string());
    let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

    println!("cargo:rustc-env=ORBCODE_GIT_SHA={sha}");
    println!("cargo:rustc-env=ORBCODE_GIT_DIRTY={dirty}");
    println!("cargo:rustc-env=ORBCODE_TARGET={target}");
    println!("cargo:rustc-env=ORBCODE_PROFILE={profile}");
    println!("cargo:rustc-env=ORBCODE_BUILD_TIMESTAMP={timestamp}");

    // The unoptimized Windows binary has substantially larger stack frames
    // than its release counterpart. Reserve the same practical headroom used
    // by Rust worker threads so the standard debug binary can reach even the
    // lightweight CLI paths (for example `--version`) without overflowing the
    // PE default main-thread stack. The reservation consumes virtual address
    // space only; Windows commits pages on demand.
    if target.ends_with("-pc-windows-msvc") {
        println!("cargo:rustc-link-arg-bin=orbcode=/STACK:8388608");
    }

    println!("cargo:rerun-if-changed=build.rs");
    if let Some(repo_root) = git_top_level() {
        println!("cargo:rerun-if-changed={repo_root}/.git/HEAD");
        println!("cargo:rerun-if-changed={repo_root}/.git/index");
    }
    println!("cargo:rerun-if-env-changed=ORBCODE_GIT_SHA_OVERRIDE");
}

fn git_short_sha() -> Option<String> {
    if let Ok(value) = std::env::var("ORBCODE_GIT_SHA_OVERRIDE")
        && !value.trim().is_empty()
    {
        return Some(value.trim().to_string());
    }
    let output = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if sha.is_empty() { None } else { Some(sha) }
}

fn git_dirty() -> bool {
    Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .is_some_and(|out| out.status.success() && !out.stdout.is_empty())
}

fn git_top_level() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if value.is_empty() { None } else { Some(value) }
}
