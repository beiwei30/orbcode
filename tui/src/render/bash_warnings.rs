pub(crate) fn destructive_bash_command_warning(command: &str) -> Option<String> {
    let lower = command.to_ascii_lowercase();
    if contains_words_in_order(&lower, &["drop", "table"])
        || contains_words_in_order(&lower, &["drop", "database"])
        || contains_words_in_order(&lower, &["drop", "schema"])
        || contains_words_in_order(&lower, &["truncate", "table"])
        || contains_words_in_order(&lower, &["truncate", "database"])
        || contains_words_in_order(&lower, &["truncate", "schema"])
    {
        return Some("Note: may drop or truncate database objects".to_string());
    }
    if contains_delete_from_without_where(&lower) {
        return Some("Note: may delete all rows from a database table".to_string());
    }
    if has_stdout_overwrite_redirection(command) {
        return Some("Note: may overwrite a file through shell redirection".to_string());
    }

    for segment in split_bash_command_segments(command) {
        let tokens = bash_words_for_warning(&segment);
        if tokens.is_empty() {
            continue;
        }
        if let Some(warning) = destructive_bash_segment_warning(&tokens) {
            return Some(warning.to_string());
        }
    }

    None
}

fn destructive_bash_segment_warning(tokens: &[String]) -> Option<&'static str> {
    let command_index = tokens
        .iter()
        .position(|token| !is_bash_warning_env_assignment(token))?;
    let command = tokens.get(command_index)?.to_ascii_lowercase();
    let args = &tokens[command_index + 1..];

    match command.as_str() {
        "git" => destructive_git_warning(args),
        "rm" => destructive_rm_warning(args),
        "chmod" if args.iter().any(|arg| recursive_flag(arg)) => {
            Some("Note: may recursively change file permissions")
        }
        "chown" if args.iter().any(|arg| recursive_flag(arg)) => {
            Some("Note: may recursively change file ownership")
        }
        "kubectl" if args.first().is_some_and(|arg| arg == "delete") => {
            Some("Note: may delete Kubernetes resources")
        }
        "terraform" if args.first().is_some_and(|arg| arg == "destroy") => {
            Some("Note: may destroy Terraform infrastructure")
        }
        _ => None,
    }
}

fn destructive_git_warning(args: &[String]) -> Option<&'static str> {
    let subcommand = args.first()?.to_ascii_lowercase();
    match subcommand.as_str() {
        "reset" if args.iter().any(|arg| arg == "--hard") => {
            Some("Note: may discard uncommitted changes")
        }
        "push"
            if args
                .iter()
                .any(|arg| matches!(arg.as_str(), "--force" | "--force-with-lease" | "-f")) =>
        {
            Some("Note: may overwrite remote history")
        }
        "clean"
            if args
                .iter()
                .any(|arg| arg == "--dry-run" || short_flag_contains(arg, 'n')) =>
        {
            None
        }
        "clean" if args.iter().any(|arg| short_flag_contains(arg, 'f')) => {
            Some("Note: may permanently delete untracked files")
        }
        "checkout" | "restore"
            if args
                .iter()
                .filter(|arg| arg.as_str() != "--")
                .any(|arg| arg == ".") =>
        {
            Some("Note: may discard all working tree changes")
        }
        "stash"
            if args.get(1).is_some_and(|arg| {
                let arg = arg.to_ascii_lowercase();
                arg == "drop" || arg == "clear"
            }) =>
        {
            Some("Note: may permanently remove stashed changes")
        }
        "branch"
            if args.iter().any(|arg| arg == "-D")
                || (args.iter().any(|arg| arg == "--delete")
                    && args.iter().any(|arg| arg == "--force")) =>
        {
            Some("Note: may force-delete a branch")
        }
        "commit" | "push" | "merge" if args.iter().any(|arg| arg == "--no-verify") => {
            Some("Note: may skip safety hooks")
        }
        "commit" if args.iter().any(|arg| arg == "--amend") => {
            Some("Note: may rewrite the last commit")
        }
        _ => None,
    }
}

fn destructive_rm_warning(args: &[String]) -> Option<&'static str> {
    let recursive = args
        .iter()
        .any(|arg| short_flag_contains(arg, 'r') || short_flag_contains(arg, 'R'));
    let force = args.iter().any(|arg| short_flag_contains(arg, 'f'));
    match (recursive, force) {
        (true, true) => Some("Note: may recursively force-remove files"),
        (true, false) => Some("Note: may recursively remove files"),
        (false, true) => Some("Note: may force-remove files"),
        (false, false) => None,
    }
}

fn split_bash_command_segments(command: &str) -> Vec<String> {
    let chars = command.chars().collect::<Vec<_>>();
    let mut segments = Vec::new();
    let mut start = 0usize;
    let mut index = 0usize;
    let mut quote = BashPreviewQuoteState::None;

    while index < chars.len() {
        let current = chars[index];
        match quote {
            BashPreviewQuoteState::Single => {
                if current == '\'' {
                    quote = BashPreviewQuoteState::None;
                }
                index += 1;
            }
            BashPreviewQuoteState::Double => {
                if current == '\\' {
                    index = index.saturating_add(2);
                } else if current == '"' {
                    quote = BashPreviewQuoteState::None;
                    index += 1;
                } else {
                    index += 1;
                }
            }
            BashPreviewQuoteState::None => {
                if current == '\\' {
                    index = index.saturating_add(2);
                } else if current == '\'' {
                    quote = BashPreviewQuoteState::Single;
                    index += 1;
                } else if current == '"' {
                    quote = BashPreviewQuoteState::Double;
                    index += 1;
                } else if matches!(current, ';' | '\n' | '|')
                    || (current == '&' && chars.get(index + 1) == Some(&'&'))
                    || (current == '&' && chars.get(index + 1) != Some(&'>'))
                {
                    let segment = chars[start..index].iter().collect::<String>();
                    if !segment.trim().is_empty() {
                        segments.push(segment);
                    }
                    index += if (current == '&' && chars.get(index + 1) == Some(&'&'))
                        || (current == '|' && chars.get(index + 1) == Some(&'|'))
                    {
                        2
                    } else {
                        1
                    };
                    start = index;
                } else {
                    index += 1;
                }
            }
        }
    }

    let segment = chars[start..].iter().collect::<String>();
    if !segment.trim().is_empty() {
        segments.push(segment);
    }
    segments
}

fn bash_words_for_warning(segment: &str) -> Vec<String> {
    let chars = segment.chars().collect::<Vec<_>>();
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote = BashPreviewQuoteState::None;
    let mut index = 0usize;

    while index < chars.len() {
        let character = chars[index];
        match quote {
            BashPreviewQuoteState::Single => {
                if character == '\'' {
                    quote = BashPreviewQuoteState::None;
                } else {
                    current.push(character);
                }
                index += 1;
            }
            BashPreviewQuoteState::Double => {
                if character == '\\' {
                    if let Some(next) = chars.get(index + 1) {
                        current.push(*next);
                    }
                    index = index.saturating_add(2);
                } else if character == '"' {
                    quote = BashPreviewQuoteState::None;
                    index += 1;
                } else {
                    current.push(character);
                    index += 1;
                }
            }
            BashPreviewQuoteState::None => {
                if character.is_whitespace() {
                    if !current.is_empty() {
                        tokens.push(std::mem::take(&mut current));
                    }
                    index += 1;
                } else if character == '\\' {
                    if let Some(next) = chars.get(index + 1) {
                        current.push(*next);
                    }
                    index = index.saturating_add(2);
                } else if character == '\'' {
                    quote = BashPreviewQuoteState::Single;
                    index += 1;
                } else if character == '"' {
                    quote = BashPreviewQuoteState::Double;
                    index += 1;
                } else {
                    current.push(character);
                    index += 1;
                }
            }
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn is_bash_warning_env_assignment(token: &str) -> bool {
    let Some((name, _value)) = token.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && name
            .chars()
            .next()
            .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
        && name
            .chars()
            .all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn short_flag_contains(arg: &str, flag: char) -> bool {
    arg.starts_with('-') && !arg.starts_with("--") && arg.chars().skip(1).any(|ch| ch == flag)
}

fn recursive_flag(arg: &str) -> bool {
    arg == "--recursive" || short_flag_contains(arg, 'r') || short_flag_contains(arg, 'R')
}

fn contains_words_in_order(value: &str, words: &[&str]) -> bool {
    let mut remaining = value;
    for word in words {
        let Some(index) = remaining.find(word) else {
            return false;
        };
        remaining = &remaining[index + word.len()..];
    }
    true
}

fn contains_delete_from_without_where(value: &str) -> bool {
    let Some(delete_index) = value.find("delete") else {
        return false;
    };
    let value = &value[delete_index..];
    let Some(from_index) = value.find("from") else {
        return false;
    };
    let statement = value[..]
        .split([';', '\n', '"', '\''])
        .next()
        .unwrap_or(&value[from_index..]);
    !statement.contains(" where ")
}

fn has_stdout_overwrite_redirection(command: &str) -> bool {
    let chars = command.chars().collect::<Vec<_>>();
    let mut quote = BashPreviewQuoteState::None;
    let mut index = 0usize;

    while index < chars.len() {
        let current = chars[index];
        match quote {
            BashPreviewQuoteState::Single => {
                if current == '\'' {
                    quote = BashPreviewQuoteState::None;
                }
                index += 1;
            }
            BashPreviewQuoteState::Double => {
                if current == '\\' {
                    index = index.saturating_add(2);
                } else if current == '"' {
                    quote = BashPreviewQuoteState::None;
                    index += 1;
                } else {
                    index += 1;
                }
            }
            BashPreviewQuoteState::None => {
                if current == '\\' {
                    index = index.saturating_add(2);
                } else if current == '\'' {
                    quote = BashPreviewQuoteState::Single;
                    index += 1;
                } else if current == '"' {
                    quote = BashPreviewQuoteState::Double;
                    index += 1;
                } else if current == '>' {
                    let previous = previous_non_whitespace(&chars, index);
                    let next = chars.get(index + 1).copied();
                    if matches!(next, Some('>' | '&' | '|')) {
                        index += 2;
                    } else if previous != Some('2') {
                        return true;
                    } else {
                        index += 1;
                    }
                } else {
                    index += 1;
                }
            }
        }
    }

    false
}

fn previous_non_whitespace(chars: &[char], index: usize) -> Option<char> {
    chars[..index]
        .iter()
        .rev()
        .find(|character| !character.is_whitespace())
        .copied()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BashPreviewQuoteState {
    None,
    Single,
    Double,
}
