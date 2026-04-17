use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{GrepoError, Result};
use crate::util::{is_valid_alias, write_atomic};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LockMode {
    Default,
    Ref { ref_name: String },
    Exact,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitLockEntry {
    pub alias: String,
    pub source: Option<String>,
    pub url: String,
    pub subdir: Option<String>,
    pub mode: LockMode,
    pub commit: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TarballLockEntry {
    pub alias: String,
    pub source: String,
    pub url: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LockEntry {
    Git(GitLockEntry),
    Tarball(TarballLockEntry),
}

impl LockEntry {
    pub fn alias(&self) -> &str {
        match self {
            Self::Git(entry) => &entry.alias,
            Self::Tarball(entry) => &entry.alias,
        }
    }

    pub fn source(&self) -> Option<&str> {
        match self {
            Self::Git(entry) => entry.source.as_deref(),
            Self::Tarball(entry) => Some(entry.source.as_str()),
        }
    }

    pub fn can_update(&self) -> bool {
        match self {
            Self::Git(entry) => !matches!(entry.mode, LockMode::Exact),
            // Tarball entries are always pinned to a concrete version; `update` re-resolves
            // the source string against the registry, which may produce a new sha.
            Self::Tarball(_) => entry_has_movable_source(self.source()),
        }
    }
}

fn entry_has_movable_source(source: Option<&str>) -> bool {
    let Some(source) = source else {
        return false;
    };
    let Some((_, spec)) = source.split_once(':') else {
        return false;
    };
    // A spec with no version (e.g. "serde") is movable; "serde@1.0.197" is pinned.
    if !spec.contains('@') {
        return true;
    }
    // Scoped npm name without version: "@types/node".
    if spec.starts_with('@') && spec[1..].matches('@').count() == 0 {
        return true;
    }
    // Dist-tag / non-exact specs we don't attempt to recognise here; treat as pinned.
    false
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Lockfile {
    repos: BTreeMap<String, LockEntry>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct StoredLockfile {
    #[serde(default)]
    repos: BTreeMap<String, StoredLockEntry>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct StoredLockEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    backend: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    subdir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mode: Option<String>,
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    ref_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sha256: Option<String>,
}

impl Lockfile {
    pub fn load(path: &Path) -> Result<Self> {
        let contents = fs::read_to_string(path)
            .map_err(|e| GrepoError::Io(format!("failed to read {}: {e}", path.display())))?;
        Self::parse(&contents)
    }

    pub fn parse(contents: &str) -> Result<Self> {
        let stored: StoredLockfile = toml::from_str(contents)?;
        let mut repos = BTreeMap::new();

        for (alias, entry) in stored.repos {
            if !is_valid_alias(&alias) {
                return Err(GrepoError::InvalidLockAlias(alias));
            }
            let lock_entry = decode_entry(alias.clone(), entry)?;
            repos.insert(alias, lock_entry);
        }

        Ok(Self { repos })
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        write_atomic(path, &self.render()?)
    }

    pub fn render(&self) -> Result<String> {
        let stored = StoredLockfile {
            repos: self
                .repos
                .iter()
                .map(|(alias, entry)| (alias.clone(), encode_entry(entry)))
                .collect(),
        };
        Ok(toml::to_string_pretty(&stored)?)
    }

    pub fn upsert(&mut self, entry: LockEntry) {
        self.repos.insert(entry.alias().to_string(), entry);
    }

    pub fn remove(&mut self, alias: &str) -> bool {
        self.repos.remove(alias).is_some()
    }

    pub fn get(&self, alias: &str) -> Option<&LockEntry> {
        self.repos.get(alias)
    }

    pub fn aliases(&self) -> Vec<String> {
        self.repos.keys().cloned().collect()
    }

    pub fn entries(&self) -> impl Iterator<Item = &LockEntry> {
        self.repos.values()
    }

    pub fn select_aliases(&self, aliases: &[String]) -> Result<Vec<String>> {
        if aliases.is_empty() {
            return Ok(self.aliases());
        }

        for alias in aliases {
            if !self.repos.contains_key(alias) {
                return Err(GrepoError::AliasNotFound(alias.clone()));
            }
        }
        Ok(aliases.to_vec())
    }
}

fn decode_entry(alias: String, stored: StoredLockEntry) -> Result<LockEntry> {
    let backend = stored.backend.as_deref().unwrap_or("git");
    match backend {
        "git" => {
            let url = stored
                .url
                .ok_or_else(|| GrepoError::LockShape(format!("{alias}: missing url")))?;
            let mode = decode_mode(&alias, stored.mode.as_deref(), stored.ref_name)?;
            Ok(LockEntry::Git(GitLockEntry {
                alias,
                source: stored.source,
                url,
                subdir: stored.subdir,
                mode,
                commit: stored.commit,
            }))
        }
        "tarball" => {
            let source = stored
                .source
                .ok_or_else(|| GrepoError::LockShape(format!("{alias}: missing source")))?;
            let url = stored
                .url
                .ok_or_else(|| GrepoError::LockShape(format!("{alias}: missing url")))?;
            let sha256 = stored
                .sha256
                .ok_or_else(|| GrepoError::LockShape(format!("{alias}: missing sha256")))?;
            Ok(LockEntry::Tarball(TarballLockEntry {
                alias,
                source,
                url,
                sha256,
            }))
        }
        other => Err(GrepoError::LockShape(format!(
            "{alias}: unknown backend \"{other}\""
        ))),
    }
}

fn decode_mode(alias: &str, mode: Option<&str>, ref_name: Option<String>) -> Result<LockMode> {
    match mode {
        Some("default") => Ok(LockMode::Default),
        Some("ref") => {
            let ref_name = ref_name.ok_or_else(|| {
                GrepoError::LockShape(format!("{alias}: mode=ref requires a ref value"))
            })?;
            Ok(LockMode::Ref { ref_name })
        }
        Some("exact") => Ok(LockMode::Exact),
        Some(other) => Err(GrepoError::LockShape(format!(
            "{alias}: unknown mode \"{other}\""
        ))),
        None => Err(GrepoError::LockShape(format!("{alias}: missing mode"))),
    }
}

fn encode_entry(entry: &LockEntry) -> StoredLockEntry {
    match entry {
        LockEntry::Git(entry) => {
            let (mode, ref_name) = match &entry.mode {
                LockMode::Default => ("default".to_string(), None),
                LockMode::Ref { ref_name } => ("ref".to_string(), Some(ref_name.clone())),
                LockMode::Exact => ("exact".to_string(), None),
            };
            StoredLockEntry {
                backend: None,
                source: entry.source.clone(),
                url: Some(entry.url.clone()),
                subdir: entry.subdir.clone(),
                mode: Some(mode),
                ref_name,
                commit: entry.commit.clone(),
                sha256: None,
            }
        }
        LockEntry::Tarball(entry) => StoredLockEntry {
            backend: Some("tarball".to_string()),
            source: Some(entry.source.clone()),
            url: Some(entry.url.clone()),
            subdir: None,
            mode: None,
            ref_name: None,
            commit: None,
            sha256: Some(entry.sha256.clone()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{GitLockEntry, LockEntry, LockMode, Lockfile, TarballLockEntry};

    #[test]
    fn parses_canonical_default_ref_and_exact_entries() {
        let lockfile = Lockfile::parse(
            r#"[repos.default_branch]
url = "git@github.com:tomrford/grepo.git"
mode = "default"
commit = "abc"

[repos.named_ref]
url = "git@github.com:tomrford/mint.git"
mode = "ref"
ref = "main"
commit = "def"

[repos.exact_pin]
url = "git@github.com:tomrford/grepo.git"
mode = "exact"
commit = "123"
"#,
        )
        .unwrap();

        assert_eq!(
            lockfile.get("default_branch"),
            Some(&LockEntry::Git(GitLockEntry {
                alias: "default_branch".into(),
                source: None,
                url: "git@github.com:tomrford/grepo.git".into(),
                subdir: None,
                mode: LockMode::Default,
                commit: Some("abc".into()),
            }))
        );
        assert_eq!(
            lockfile.get("named_ref"),
            Some(&LockEntry::Git(GitLockEntry {
                alias: "named_ref".into(),
                source: None,
                url: "git@github.com:tomrford/mint.git".into(),
                subdir: None,
                mode: LockMode::Ref {
                    ref_name: "main".into(),
                },
                commit: Some("def".into()),
            }))
        );
        assert_eq!(
            lockfile.get("exact_pin"),
            Some(&LockEntry::Git(GitLockEntry {
                alias: "exact_pin".into(),
                source: None,
                url: "git@github.com:tomrford/grepo.git".into(),
                subdir: None,
                mode: LockMode::Exact,
                commit: Some("123".into()),
            }))
        );
    }

    #[test]
    fn renders_canonical_git_format_unchanged_when_no_source_or_subdir() {
        let lockfile = Lockfile::parse(
            r#"[repos.default_branch]
url = "git@github.com:tomrford/grepo.git"
mode = "default"
commit = "abc"

[repos.named_ref]
url = "git@github.com:tomrford/mint.git"
mode = "ref"
ref = "default"
commit = "def"
"#,
        )
        .unwrap();

        assert_eq!(
            lockfile.render().unwrap(),
            r#"[repos.default_branch]
url = "git@github.com:tomrford/grepo.git"
mode = "default"
commit = "abc"

[repos.named_ref]
url = "git@github.com:tomrford/mint.git"
mode = "ref"
ref = "default"
commit = "def"
"#
        );
    }

    #[test]
    fn parses_and_renders_git_entry_with_source_and_subdir() {
        let input = r#"[repos.react]
source = "npm:react@18.2.0"
url = "https://github.com/facebook/react.git"
subdir = "packages/react"
mode = "exact"
commit = "be229c5655074642ee664f532f2e7411dd7dccc7"
"#;
        let lockfile = Lockfile::parse(input).unwrap();

        assert_eq!(
            lockfile.get("react"),
            Some(&LockEntry::Git(GitLockEntry {
                alias: "react".into(),
                source: Some("npm:react@18.2.0".into()),
                url: "https://github.com/facebook/react.git".into(),
                subdir: Some("packages/react".into()),
                mode: LockMode::Exact,
                commit: Some("be229c5655074642ee664f532f2e7411dd7dccc7".into()),
            }))
        );
        assert_eq!(lockfile.render().unwrap(), input);
    }

    #[test]
    fn parses_and_renders_tarball_entry() {
        let input = r#"[repos.serde]
backend = "tarball"
source = "cargo:serde@1.0.197"
url = "https://crates.io/api/v1/crates/serde/1.0.197/download"
sha256 = "3fb1c873e1b9b056a4dc4c0c198b24c3ffa059243875552b2bd0933b1aee4ce2"
"#;
        let lockfile = Lockfile::parse(input).unwrap();

        assert_eq!(
            lockfile.get("serde"),
            Some(&LockEntry::Tarball(TarballLockEntry {
                alias: "serde".into(),
                source: "cargo:serde@1.0.197".into(),
                url: "https://crates.io/api/v1/crates/serde/1.0.197/download".into(),
                sha256: "3fb1c873e1b9b056a4dc4c0c198b24c3ffa059243875552b2bd0933b1aee4ce2".into(),
            }))
        );
        assert_eq!(lockfile.render().unwrap(), input);
    }

    #[test]
    fn rejects_old_track_format() {
        let error = Lockfile::parse(
            r#"[repos.default_branch]
url = "git@github.com:tomrford/grepo.git"
track = "default"
commit = "abc"
"#,
        )
        .unwrap_err();

        let msg = format!("{error}");
        assert!(
            msg.contains("missing mode") || msg.contains("invalid grepo/.lock TOML"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn rejects_invalid_aliases_in_lockfile_with_manifest_context() {
        let error = Lockfile::parse(
            r#"[repos.".bad"]
url = "git@github.com:tomrford/grepo.git"
mode = "default"
"#,
        )
        .unwrap_err();

        assert_eq!(error.to_string(), "invalid alias in grepo/.lock: .bad");
    }
}
