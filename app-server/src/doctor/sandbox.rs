use std::path::Path;
#[cfg(any(target_os = "linux", target_os = "windows"))]
use std::path::PathBuf;

use orbcode_protocol::SandboxMode;

use super::{DoctorCheck, DoctorStatus};

pub(super) fn sandbox_check(mode: SandboxMode, allow_network: bool) -> DoctorCheck {
    match mode {
        SandboxMode::DangerFullAccess => DoctorCheck {
            name: "sandbox".to_string(),
            status: DoctorStatus::Warn,
            detail: "danger-full-access; Bash commands run without OS sandboxing".to_string(),
        },
        SandboxMode::WorkspaceWrite | SandboxMode::ReadOnly => {
            restrictive_sandbox_check(mode, allow_network)
        }
        _ => restrictive_sandbox_check(mode, allow_network),
    }
}

#[cfg(target_os = "macos")]
fn restrictive_sandbox_check(mode: SandboxMode, allow_network: bool) -> DoctorCheck {
    if Path::new("/usr/bin/sandbox-exec").exists() {
        let network = if allow_network {
            "network allowed"
        } else {
            "network blocked"
        };
        DoctorCheck {
            name: "sandbox".to_string(),
            status: DoctorStatus::Pass,
            detail: format!(
                "{} enforced for Bash with macOS seatbelt; {network}",
                mode.as_str(),
            ),
        }
    } else {
        DoctorCheck {
            name: "sandbox".to_string(),
            status: DoctorStatus::Fail,
            detail: format!(
                "{} configured, but /usr/bin/sandbox-exec was not found",
                mode.as_str()
            ),
        }
    }
}

#[cfg(target_os = "linux")]
fn restrictive_sandbox_check(mode: SandboxMode, allow_network: bool) -> DoctorCheck {
    if executable_in_path("bwrap").is_some() {
        let network = if allow_network {
            "network allowed"
        } else {
            "network blocked"
        };
        DoctorCheck {
            name: "sandbox".to_string(),
            status: DoctorStatus::Pass,
            detail: format!(
                "{} enforced for Bash with Linux bubblewrap; {network}",
                mode.as_str(),
            ),
        }
    } else {
        DoctorCheck {
            name: "sandbox".to_string(),
            status: DoctorStatus::Fail,
            detail: format!(
                "{} configured, but Linux bubblewrap (`bwrap`) was not found in PATH",
                mode.as_str()
            ),
        }
    }
}

#[cfg(target_os = "windows")]
fn restrictive_sandbox_check(mode: SandboxMode, allow_network: bool) -> DoctorCheck {
    let runner = std::env::var_os("ORBCODE_WINDOWS_SANDBOX_RUNNER")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .or_else(|| executable_in_path("orbcode-windows-sandbox-runner"));
    if runner.is_some() {
        let network = if allow_network {
            "network allowed"
        } else {
            "network blocked"
        };
        DoctorCheck {
            name: "sandbox".to_string(),
            status: DoctorStatus::Warn,
            detail: format!(
                "{} configured for Bash with Windows sandbox runner POC; {network}",
                mode.as_str(),
            ),
        }
    } else {
        DoctorCheck {
            name: "sandbox".to_string(),
            status: DoctorStatus::Fail,
            detail: format!(
                "{} configured, but Windows sandbox runner POC requires `orbcode-windows-sandbox-runner` on PATH or ORBCODE_WINDOWS_SANDBOX_RUNNER",
                mode.as_str()
            ),
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn restrictive_sandbox_check(mode: SandboxMode, _allow_network: bool) -> DoctorCheck {
    DoctorCheck {
        name: "sandbox".to_string(),
        status: DoctorStatus::Fail,
        detail: format!(
            "{} configured, but Bash sandbox enforcement is only implemented for macOS seatbelt, Linux bubblewrap, and the Windows sandbox runner backend right now",
            mode.as_str()
        ),
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn executable_in_path(binary: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join(binary))
            .find(|path| path.is_file())
    })
}
