use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{GrepoError, Result};
use crate::util::{is_valid_alias, write_atomic};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum LockMode {
    Default,
    Ref {
        #[serde(rename = "ref")]
        ref_name: String,
    },
    Exact,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LockEntry {
    pub alias: String,
    pub url: String,
    pub mode: LockMode,
    pub commit: Option<String>,
}

impl LockEntry {
    pub fn new(alias: String, url: String) -> Self {
        Self {
            alias,
            url,
            mode: LockMode::Exact,
            commit: None,
        }
    }

    pub fn can_update(&self) -> bool {
        !matches!(self.mode, LockMode::Exact)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Lockfile {
    repos: BTreeMap<String, LockEntry>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
struct StoredLockfile {
    #[serde(default)]
    repos: BTreeMap<String, StoredLockEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct StoredLockEntry {
    url: String,
    #[serde(flatten)]
    mode: LockMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    commit: Option<String>,
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

            repos.insert(
                alias.clone(),
                LockEntry {
                    alias,
                    url: entry.url,
                    mode: entry.mode,
                    commit: entry.commit,
                },
            );
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
                .map(|(alias, entry)| {
                    (
                        alias.clone(),
                        StoredLockEntry {
                            url: entry.url.clone(),
                            mode: entry.mode.clone(),
                            commit: entry.commit.clone(),
                        },
                    )
                })
                .collect(),
        };
        Ok(toml::to_string_pretty(&stored)?)
    }

    pub fn upsert(&mut self, entry: LockEntry) {
        self.repos.insert(entry.alias.clone(), entry);
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

#[cfg(test)]
mod tests {
    use super::{LockEntry, LockMode, Lockfile};

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
            Some(&LockEntry {
                alias: "default_branch".into(),
                url: "git@github.com:tomrford/grepo.git".into(),
                mode: LockMode::Default,
                commit: Some("abc".into()),
            })
        );
        assert_eq!(
            lockfile.get("named_ref"),
            Some(&LockEntry {
                alias: "named_ref".into(),
                url: "git@github.com:tomrford/mint.git".into(),
                mode: LockMode::Ref {
                    ref_name: "main".into(),
                },
                commit: Some("def".into()),
            })
        );
        assert_eq!(
            lockfile.get("exact_pin"),
            Some(&LockEntry {
                alias: "exact_pin".into(),
                url: "git@github.com:tomrford/grepo.git".into(),
                mode: LockMode::Exact,
                commit: Some("123".into()),
            })
        );
    }

    #[test]
    fn renders_canonical_mode_format() {
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
    fn rejects_old_track_format() {
        let error = Lockfile::parse(
            r#"[repos.default_branch]
url = "git@github.com:tomrford/grepo.git"
track = "default"
commit = "abc"
"#,
        )
        .unwrap_err();

        assert!(format!("{error}").contains("invalid grepo/.lock TOML"));
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
