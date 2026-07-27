//! Discovery and loading of the on-disk golden fixtures shared between the
//! TypeScript reference implementation and the Rust port. Fixtures live under
//! `compat-fixtures/fixtures/<category>/` and are grouped by the behavioral
//! surface they exercise.

use std::fs;
use std::path::{Path, PathBuf};

/// The fixture categories. Each maps to a subdirectory under [`fixtures_root`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixtureCategory {
    /// Provider request bodies / streaming exchanges (Anthropic, OpenAI).
    ProviderStreams,
    /// Full TypeScript-style session transcripts (`.jsonl`).
    Transcripts,
    /// Isolated tool_use / tool_result block exchanges.
    ToolCalls,
    /// Permission prompt request payloads.
    Permissions,
    /// Headless stream-json event sequences (normalized NDJSON goldens).
    StreamJson,
}

impl FixtureCategory {
    pub fn dir_name(self) -> &'static str {
        match self {
            Self::ProviderStreams => "provider_streams",
            Self::Transcripts => "transcripts",
            Self::ToolCalls => "tool_calls",
            Self::Permissions => "permissions",
            Self::StreamJson => "stream_json",
        }
    }

    pub const ALL: [FixtureCategory; 5] = [
        FixtureCategory::ProviderStreams,
        FixtureCategory::Transcripts,
        FixtureCategory::ToolCalls,
        FixtureCategory::Permissions,
        FixtureCategory::StreamJson,
    ];
}

/// A single loaded fixture file.
#[derive(Clone, Debug)]
pub struct Fixture {
    /// File stem (name without extension), used as the fixture identifier.
    pub name: String,
    /// Absolute path to the fixture on disk.
    pub path: PathBuf,
    /// Raw file contents.
    pub contents: String,
}

/// Absolute path to the `fixtures/` directory bundled with this crate. Resolves
/// relative to the crate manifest so it works regardless of the test runner's
/// current working directory.
pub fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

/// Absolute path to a category subdirectory.
pub fn category_dir(category: FixtureCategory) -> PathBuf {
    fixtures_root().join(category.dir_name())
}

/// Load every fixture file directly inside a category directory, sorted by name
/// for deterministic iteration. Subdirectories (such as `transcripts/corrupt/`)
/// are not recursed into.
pub fn load_category(category: FixtureCategory) -> Vec<Fixture> {
    load_dir(&category_dir(category))
}

/// Load every fixture file directly inside an arbitrary directory under the
/// fixtures root.
pub fn load_dir(dir: &Path) -> Vec<Fixture> {
    let mut fixtures = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => panic!("failed to read fixture dir {}: {error}", dir.display()),
    };
    for entry in entries {
        let entry = entry.expect("read fixture dir entry");
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .expect("fixture file stem")
            .to_string();
        let contents = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read fixture {}: {error}", path.display()));
        fixtures.push(Fixture {
            name,
            path,
            contents,
        });
    }
    fixtures.sort_by(|a, b| a.name.cmp(&b.name));
    fixtures
}

/// Load one fixture by category and name (without extension). Returns `None`
/// when no matching file exists.
pub fn load_named(category: FixtureCategory, name: &str) -> Option<Fixture> {
    load_category(category)
        .into_iter()
        .find(|fixture| fixture.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compat_fixtures_root_exists() {
        assert!(
            fixtures_root().is_dir(),
            "fixtures root should exist at {}",
            fixtures_root().display()
        );
    }

    #[test]
    fn compat_every_category_has_at_least_three_fixtures() {
        for category in FixtureCategory::ALL {
            let count = load_category(category).len();
            assert!(
                count >= 3,
                "category {} should have >= 3 fixtures, found {count}",
                category.dir_name()
            );
        }
    }

    #[test]
    fn compat_total_fixture_count_meets_minimum() {
        let total: usize = FixtureCategory::ALL
            .iter()
            .map(|category| load_category(*category).len())
            .sum();
        assert!(total >= 12, "expected >= 12 fixtures total, found {total}");
    }
}
