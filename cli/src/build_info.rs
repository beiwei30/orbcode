//! Build-time metadata embedded into the orbcode binary.
//!
//! Values come from `build.rs`: git SHA, build target, profile, and a UTC
//! timestamp. Surfaced through `--version` and the `doctor` report so
//! packaged binaries are traceable in the field.

pub const PACKAGE_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const GIT_SHA: &str = env!("ORBCODE_GIT_SHA");
pub const GIT_DIRTY: &str = env!("ORBCODE_GIT_DIRTY");
pub const TARGET: &str = env!("ORBCODE_TARGET");
pub const PROFILE: &str = env!("ORBCODE_PROFILE");
pub const BUILD_TIMESTAMP: &str = env!("ORBCODE_BUILD_TIMESTAMP");

pub const PROVIDERS: &str = "anthropic,openai,gemini,grok";

/// Single-line `-V` string. Format: `<package> (<sha>[+dirty] <target>)`.
pub const VERSION_LINE: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("ORBCODE_GIT_SHA"),
    env!("ORBCODE_GIT_DIRTY"),
    " ",
    env!("ORBCODE_TARGET"),
    ")"
);

/// Multi-line `--version` body wired into clap's `long_version`. Mirrors the
/// `build_info` doctor check so `orbcode --version` and `orbcode doctor` stay in
/// agreement.
pub const LONG_VERSION_LINE: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("ORBCODE_GIT_SHA"),
    env!("ORBCODE_GIT_DIRTY"),
    ")\n",
    "target:    ",
    env!("ORBCODE_TARGET"),
    "\n",
    "profile:   ",
    env!("ORBCODE_PROFILE"),
    "\n",
    "built:     ",
    env!("ORBCODE_BUILD_TIMESTAMP"),
    "\n",
    "providers: anthropic,openai,gemini,grok",
);

/// Single-line summary used by the `build_info` doctor check. Keeping it on
/// one line means the standard `STATUS NAME DETAIL` doctor row format keeps
/// working without the detail being reflowed across screen width.
pub fn doctor_detail() -> String {
    format!(
        "version={PACKAGE_VERSION} sha={GIT_SHA}{GIT_DIRTY} target={TARGET} profile={PROFILE} \
         built={BUILD_TIMESTAMP} providers={PROVIDERS}",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_line_contains_target() {
        assert!(VERSION_LINE.contains(PACKAGE_VERSION));
        assert!(VERSION_LINE.contains(TARGET));
    }

    #[test]
    fn long_version_line_contains_providers_and_target() {
        assert!(LONG_VERSION_LINE.contains(TARGET));
        assert!(LONG_VERSION_LINE.contains("anthropic"));
        assert!(LONG_VERSION_LINE.contains("openai"));
        assert!(LONG_VERSION_LINE.contains("gemini"));
        assert!(LONG_VERSION_LINE.contains("grok"));
    }

    #[test]
    fn doctor_detail_is_single_line() {
        let detail = doctor_detail();
        assert!(!detail.contains('\n'), "doctor detail must be one line");
        assert!(detail.contains("version="));
        assert!(detail.contains("target="));
        assert!(detail.contains("providers="));
    }
}
