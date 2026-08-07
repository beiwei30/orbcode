# Orbcode Desktop macOS Release Smoke

This checklist is the versioned clean-machine gate for the first supported
desktop target. Run it against the exact signed and notarized `.app` produced
for a release; do not substitute a development build or external helper.

## Build, sign, and notarize

1. On a clean macOS build worker, install the pinned Rust and Node dependencies.
2. Run `npm ci` in `clients/desktop`.
3. Run `npm run build:release-app` in `clients/desktop`. This stages the
   workspace-matching release `orbcode` helper and produces the `.app`.
4. Store notarization credentials in an Apple keychain profile, set
   `APPLE_SIGNING_IDENTITY` and `APPLE_NOTARY_KEYCHAIN_PROFILE`, then run
   `scripts/sign-notarize-desktop.sh`.
5. Run `scripts/audit-desktop-artifact.sh` on the stapled app. Inspect both the
   outer app and `Contents/Resources/bin/orbcode` with `codesign -dv --verbose=4`.

The release uses Hardened Runtime with an empty entitlement set. It is not App
Sandboxed: the signed helper must access the selected repositories, SSH, and
provider endpoints. V1 has no updater and never downloads an executable.

## Clean-machine product smoke

1. Copy the notarized archive to a Mac with no repository checkout or Orbcode
   development environment. Expand it and move the app into `/Applications`.
2. Confirm Gatekeeper opens it without an override and
   `spctl --assess --type execute` succeeds.
3. Connect locally, create a session, complete one mock/staging turn, approve
   and deny separate tool requests, cancel a streaming turn, then reconnect and
   resume the transcript.
4. Rename, fork, and delete a disposable session. Change every unlocked session
   control and confirm a managed-locked setting is disabled.
5. Save an SSH profile and inspect browser storage: it may contain only the
   profile name, target, remote cwd, and remote Orbcode path. Connect to a host
   running the exact release version, verify host-key behavior, complete a turn,
   interrupt the network, and confirm OpenSSH reaches the resumable disconnect
   state within its configured keepalive bound. Reconnect and resume. Repeat
   across laptop sleep/wake: a healthy connection may continue, while an
   unreachable peer must follow the same bounded resumable path.
6. Leave a permission request open and quit the app. Confirm the request is
   cancelled and neither the local helper nor `/usr/bin/ssh` remains running.
7. Force-kill the helper and confirm the UI reports a resumable connection exit
   without exposing stderr, argv, prompts, environment, or tokens.
8. Delete the app from `/Applications`. Confirm no privileged helper, launch
   agent, updater, system extension, or background service was installed.

## Release record

Record the release tag, commit, app/helper versions and designated requirements,
app/helper Hardened Runtime flags and entitlements, notary submission ID,
Gatekeeper result, test job URL, macOS version/hardware, local and SSH smoke
results (including network loss and sleep/wake), and the operator/date. A
release is blocked if any field is absent.
