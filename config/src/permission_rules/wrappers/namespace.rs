use super::super::wrapper_spec::{self, common_aliases};

pub(in crate::permission_rules) const CHROOT_SPEC: wrapper_spec::WrapperSpec =
    wrapper_spec::WrapperSpec {
        long_flags: &["--skip-chdir"],
        long_options_with_value: &["--userspec", "--groups"],
        inline_for_all_long_options: true,
        positional_args_before_command: 1,
        ..wrapper_spec::WrapperSpec::with_aliases(&[
            "chroot",
            "/bin/chroot",
            "/usr/bin/chroot",
            "/usr/sbin/chroot",
            "/usr/local/bin/chroot",
            "/opt/homebrew/bin/chroot",
        ])
    };

pub(in crate::permission_rules) const UNSHARE_SPEC: wrapper_spec::WrapperSpec =
    wrapper_spec::WrapperSpec {
        long_flags: &[
            "--mount",
            "--uts",
            "--ipc",
            "--net",
            "--pid",
            "--user",
            "--cgroup",
            "--time",
            "--fork",
            "--map-root-user",
            "--map-current-user",
            "--map-auto",
            "--mount-proc",
            "--keep-caps",
        ],
        short_flag_chars: "muinpUCTfrc",
        long_options_with_value: &[
            "--root",
            "--wd",
            "--setuid",
            "--setgid",
            "--map-user",
            "--map-group",
            "--setgroups",
            "--propagation",
            "--kill-child",
            "--monotonic",
            "--boottime",
        ],
        short_options_with_value: &["-R", "-w", "-S", "-G"],
        // long_options_with_value mirrored automatically; --mount-proc is a
        // long_flag that also accepts inline `--mount-proc=/proc` form.
        inline_value_long_prefixes: &["--mount-proc"],
        inline_for_all_long_options: true,
        inline_for_all_short_options: true,
        ..wrapper_spec::WrapperSpec::with_aliases(common_aliases!("unshare"))
    };

pub(in crate::permission_rules) const NSENTER_SPEC: wrapper_spec::WrapperSpec =
    wrapper_spec::WrapperSpec {
        long_flags: &[
            "--all",
            "--mount",
            "--uts",
            "--ipc",
            "--net",
            "--pid",
            "--cgroup",
            "--user",
            "--time",
            "--no-fork",
            "--follow-context",
            "--preserve-credentials",
        ],
        short_flag_chars: "amuinpCUTFZ",
        long_options_with_value: &[
            "--target",
            "--setuid",
            "--setgid",
            "--root",
            "--wd",
            "--wdns",
            "--join-cgroup",
        ],
        short_options_with_value: &["-t", "-S", "-G", "-r", "-w", "-W"],
        // long_options_with_value mirrored automatically; the namespace
        // long_flags (--mount/--uts/--ipc/--net/--pid/--cgroup/--user/--time)
        // also accept inline `--mount=PATH` form when used to pass a path.
        inline_value_long_prefixes: &[
            "--mount", "--uts", "--ipc", "--net", "--pid", "--cgroup", "--user", "--time",
        ],
        inline_for_all_long_options: true,
        inline_for_all_short_options: true,
        ..wrapper_spec::WrapperSpec::with_aliases(common_aliases!("nsenter"))
    };

pub(in crate::permission_rules) const BUBBLEWRAP_SPEC: wrapper_spec::WrapperSpec =
    wrapper_spec::WrapperSpec {
        long_flags: &[
            "--unshare-all",
            "--unshare-user",
            "--unshare-user-try",
            "--unshare-ipc",
            "--unshare-pid",
            "--unshare-net",
            "--unshare-uts",
            "--unshare-cgroup",
            "--share-net",
            "--die-with-parent",
            "--new-session",
            "--as-pid-1",
            "--clearenv",
        ],
        long_options_with_value: &[
            "--dev",
            "--proc",
            "--tmpfs",
            "--mqueue",
            "--dir",
            "--chdir",
            "--setuid",
            "--setgid",
            "--uid",
            "--gid",
            "--hostname",
            "--unsetenv",
            "--perms",
            "--remount-ro",
            "--unshare-user-fd",
            "--sync-fd",
            "--info-fd",
            "--seccomp",
        ],
        long_options_with_two_values: &[
            "--bind",
            "--ro-bind",
            "--dev-bind",
            "--bind-try",
            "--ro-bind-try",
            "--dev-bind-try",
            "--symlink",
            "--file",
            "--ro-bind-data",
            "--bind-data",
            "--chmod",
            "--setenv",
        ],
        ..wrapper_spec::WrapperSpec::with_aliases(&[
            "bwrap",
            "bubblewrap",
            "/usr/bin/bwrap",
            "/usr/bin/bubblewrap",
            "/usr/local/bin/bwrap",
            "/usr/local/bin/bubblewrap",
            "/opt/homebrew/bin/bwrap",
            "/opt/homebrew/bin/bubblewrap",
        ])
    };

pub(in crate::permission_rules) const FIREJAIL_SPEC: wrapper_spec::WrapperSpec =
    wrapper_spec::WrapperSpec {
        long_flags: &[
            "--quiet",
            "--noprofile",
            "--private",
            "--net=none",
            "--noroot",
            "--nonewprivs",
            "--nodbus",
            "--nosound",
            "--x11=none",
            "--ipc-namespace",
            "--netns",
            "--overlay",
            "--read-only",
            "--seccomp",
            "--shell=none",
        ],
        long_options_with_value: &[
            "--profile",
            "--name",
            "--hostname",
            "--private-home",
            "--private-tmp",
            "--private-dev",
            "--net",
            "--dns",
            "--blacklist",
            "--whitelist",
            "--read-write",
            "--tmpfs",
            "--env",
            "--rlimit-nproc",
            "--rlimit-fsize",
            "--rlimit-as",
            "--caps",
            "--protocol",
            "--apparmor",
        ],
        // long_options_with_value mirrored automatically; --private / --read-only
        // / --seccomp are long_flags that also accept `=VALUE`.
        inline_value_long_prefixes: &["--private", "--read-only", "--seccomp"],
        inline_for_all_long_options: true,
        ..wrapper_spec::WrapperSpec::with_aliases(common_aliases!("firejail"))
    };

pub(in crate::permission_rules) const SYSTEMD_RUN_SPEC: wrapper_spec::WrapperSpec =
    wrapper_spec::WrapperSpec {
        long_flags: &[
            "--user",
            "--system",
            "--scope",
            "--slice-inherit",
            "--collect",
            "--remain-after-exit",
            "--send-sighup",
            "--same-dir",
            "--pty",
            "--pipe",
            "--wait",
            "--no-block",
            "--quiet",
            "--service-type=exec",
            "--service-type=simple",
            "--service-type=oneshot",
        ],
        long_options_with_value: &[
            "--unit",
            "--description",
            "--slice",
            "--property",
            "--setenv",
            "--uid",
            "--gid",
            "--nice",
            "--working-directory",
            "--service-type",
            "--timer-property",
            "--on-active",
            "--on-boot",
            "--on-startup",
            "--on-unit-active",
            "--on-unit-inactive",
            "--calendar",
        ],
        short_options_with_value: &["-p", "-E"],
        inline_for_all_long_options: true,
        inline_for_all_short_options: true,
        ..wrapper_spec::WrapperSpec::with_aliases(common_aliases!("systemd-run"))
    };
