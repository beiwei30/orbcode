use crate::tests::support::*;

fn preset_options() -> Vec<PermissionPresetOption> {
    vec![
        PermissionPresetOption {
            value: ModelPermissionPreset::AskForApproval,
            label: "Ask for approval".to_string(),
            description: "Orb Code can read and edit files in the current workspace, and run commands. Approval is required to access the internet or edit other files.".to_string(),
            current: true,
            disabled_reason: None,
        },
        PermissionPresetOption {
            value: ModelPermissionPreset::ApproveForMe,
            label: "Approve for me".to_string(),
            description: "Only ask for actions detected as potentially unsafe.".to_string(),
            current: false,
            disabled_reason: None,
        },
        PermissionPresetOption {
            value: ModelPermissionPreset::FullAccess,
            label: "Full Access".to_string(),
            description: "Orb Code can edit files outside this workspace and access the internet without asking for approval. Exercise caution when using.".to_string(),
            current: false,
            disabled_reason: None,
        },
    ]
}

#[test]
fn permission_picker_shows_three_presets_and_current_state() {
    let mut picker = PermissionPickerState::new("/permissions", preset_options());
    let rendered = plain_text_lines(picker.cached_lines(100)).join("\n");

    assert!(rendered.contains("Ask for approval (current)"));
    assert!(rendered.contains("Approve for me"));
    assert!(rendered.contains("Full Access"));
    assert!(!rendered.contains("Add a new rule"));
}

#[test]
fn permission_picker_selects_non_dangerous_preset_immediately() {
    let mut picker = PermissionPickerState::new("/permissions", preset_options());
    picker.selected = 1;

    assert_eq!(
        apply_permission_picker_key(
            &mut picker,
            &KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        ),
        PermissionPickerKeyAction::SetPreset {
            command: "/permissions".to_string(),
            preset: ModelPermissionPreset::ApproveForMe,
        }
    );
}

#[test]
fn full_access_requires_a_second_confirmation() {
    let mut picker = PermissionPickerState::new("/permissions", preset_options());
    picker.selected = 2;

    assert!(matches!(
        apply_permission_picker_key(
            &mut picker,
            &KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        ),
        PermissionPickerKeyAction::Status(_)
    ));
    assert!(picker.confirming_full_access);
    assert!(matches!(
        apply_permission_picker_key(
            &mut picker,
            &KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        ),
        PermissionPickerKeyAction::Status(message) if message.contains("cancelled")
    ));
    assert!(!picker.confirming_full_access);
    assert!(matches!(
        apply_permission_picker_key(
            &mut picker,
            &KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        ),
        PermissionPickerKeyAction::Status(_)
    ));
    assert_eq!(
        apply_permission_picker_key(
            &mut picker,
            &KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        ),
        PermissionPickerKeyAction::SetPreset {
            command: "/permissions".to_string(),
            preset: ModelPermissionPreset::FullAccess,
        }
    );
}

#[test]
fn permission_picker_wraps_narrow_descriptions_without_losing_state() {
    let mut options = preset_options();
    options[2].disabled_reason = Some("Disabled by managed policy".to_string());
    let mut picker = PermissionPickerState::new("/permissions", options);
    picker.selected = 2;

    let rendered = plain_text_lines(picker.cached_lines(24)).join("\n");
    for word in "Orb Code can read and edit files in the current workspace, and run commands. Approval is required to access the internet or edit other files."
        .split_whitespace()
    {
        assert!(rendered.contains(word), "missing `{word}`:\n{rendered}");
    }
    assert!(rendered.contains("(current)"));
    assert!(rendered.contains("(disabled)"));
    for word in "Disabled by managed policy".split_whitespace() {
        assert!(rendered.contains(word), "missing `{word}`:\n{rendered}");
    }
}

#[test]
fn permission_picker_renders_the_three_option_layout_at_80_columns() {
    let options = preset_options();
    let descriptions = options
        .iter()
        .map(|option| option.description.clone())
        .collect::<Vec<_>>();
    let mut picker = PermissionPickerState::new("/permissions", options);
    let lines = plain_text_lines(picker.cached_lines(80));
    let rendered = lines.join("\n");

    assert_eq!(
        lines.first().map(String::as_str),
        Some("Update Model Permissions")
    );
    assert!(lines.iter().all(|line| display_width_str(line) <= 80));
    for label in [
        "1. Ask for approval (current)",
        "2. Approve for me",
        "3. Full Access",
    ] {
        assert!(rendered.contains(label), "missing `{label}`:\n{rendered}");
    }
    for description in descriptions {
        for word in description.split_whitespace() {
            assert!(rendered.contains(word), "missing `{word}`:\n{rendered}");
        }
    }
}

#[test]
fn disabled_full_access_explains_managed_policy() {
    let mut options = preset_options();
    options[2].disabled_reason = Some(
        "Full Access is disabled by managed policy (disable_bypass_permissions_mode).".to_string(),
    );
    let mut picker = PermissionPickerState::new("/permissions", options);
    picker.selected = 2;

    let action = apply_permission_picker_key(
        &mut picker,
        &KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    );
    assert!(
        matches!(action, PermissionPickerKeyAction::Status(message) if message.contains("managed policy"))
    );
    assert!(!picker.confirming_full_access);
}

#[test]
fn permission_picker_outer_height_wraps_border() {
    let mut picker = PermissionPickerState::new("/permissions", preset_options());
    let line_count = picker.cached_lines(80).len();
    assert_eq!(
        permission_picker_outer_height(line_count),
        u16::try_from(line_count + 2).unwrap()
    );
}
