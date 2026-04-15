use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use crate::util::{Result, err, is_valid_alias, write_atomic};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TrackMode {
    DefaultBranch,
    Pinned,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LockEntry {
    pub alias: String,
    pub url: String,
    pub ref_name: Option<String>,
    pub track: TrackMode,
    pub commit: Option<String>,
}

impl LockEntry {
    pub fn new(alias: String, url: String) -> Self {
        Self {
            alias,
            url,
            ref_name: None,
            track: TrackMode::Pinned,
            commit: None,
        }
    }

    pub fn can_update(&self) -> bool {
        self.track == TrackMode::DefaultBranch || self.ref_name.is_some()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Lockfile {
    repos: BTreeMap<String, LockEntry>,
}

impl Lockfile {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn load(path: &Path) -> Result<Self> {
        let contents = fs::read_to_string(path)
            .map_err(|error| err(format!("failed to read {}: {error}", path.display())))?;
        Self::parse(&contents)
    }

    pub fn parse(contents: &str) -> Result<Self> {
        let mut lockfile = Self::empty();
        let mut current_alias: Option<String> = None;
        let mut current_entry: Option<LockEntry> = None;

        for (index, raw_line) in contents.lines().enumerate() {
            let line_number = index + 1;
            let trimmed = raw_line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            if trimmed.starts_with('[') {
                finish_section(&mut lockfile, current_alias.take(), current_entry.take())?;
                let alias = parse_section_header(trimmed, line_number)?;
                current_entry = Some(LockEntry::new(alias.clone(), String::new()));
                current_alias = Some(alias);
                continue;
            }

            let entry = current_entry.as_mut().ok_or_else(|| {
                err(format!(
                    "invalid grepo/.lock line {}: expected a [repos.<alias>] section first",
                    line_number
                ))
            })?;
            let (key, value) = trimmed.split_once('=').ok_or_else(|| {
                err(format!(
                    "invalid grepo/.lock line {}: expected key = \"value\"",
                    line_number
                ))
            })?;
            let key = key.trim();
            let value = parse_quoted_value(value.trim(), line_number)?;

            match key {
                "url" => entry.url = value,
                "ref" => entry.ref_name = Some(value),
                "track" => {
                    if value != "default" {
                        return Err(err(format!(
                            "invalid track value on line {}: {}",
                            line_number, value
                        )));
                    }
                    entry.track = TrackMode::DefaultBranch;
                }
                "commit" => entry.commit = Some(value),
                _ => {
                    return Err(err(format!(
                        "unsupported key on line {}: {}",
                        line_number, key
                    )));
                }
            }
        }

        finish_section(&mut lockfile, current_alias, current_entry)?;
        Ok(lockfile)
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        write_atomic(path, &self.render())
    }

    pub fn render(&self) -> String {
        let mut output = String::new();
        for entry in self.repos.values() {
            output.push_str(&format!("[repos.{}]\n", entry.alias));
            output.push_str(&format!(r#"url = "{}""#, escape(&entry.url)));
            output.push('\n');
            if let Some(ref_name) = &entry.ref_name {
                output.push_str(&format!(r#"ref = "{}""#, escape(ref_name)));
                output.push('\n');
            } else if entry.track == TrackMode::DefaultBranch {
                output.push_str(r#"track = "default""#);
                output.push('\n');
            }
            if let Some(commit) = &entry.commit {
                output.push_str(&format!(r#"commit = "{}""#, escape(commit)));
                output.push('\n');
            }
            output.push('\n');
        }
        output
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

    pub fn alias_set(&self) -> BTreeSet<String> {
        self.repos.keys().cloned().collect()
    }

    pub fn select_aliases(&self, aliases: &[String]) -> Result<Vec<String>> {
        if aliases.is_empty() {
            return Ok(self.aliases());
        }

        for alias in aliases {
            if !self.repos.contains_key(alias) {
                return Err(err(format!("alias not found in grepo/.lock: {alias}")));
            }
        }
        Ok(aliases.to_vec())
    }
}

fn finish_section(
    lockfile: &mut Lockfile,
    alias: Option<String>,
    entry: Option<LockEntry>,
) -> Result<()> {
    let (Some(alias), Some(entry)) = (alias, entry) else {
        return Ok(());
    };

    if !is_valid_alias(&alias) {
        return Err(err(format!("invalid alias in grepo/.lock: {alias}")));
    }
    if entry.url.is_empty() {
        return Err(err(format!("alias {} is missing a url", alias)));
    }
    if entry.ref_name.is_some() && entry.track == TrackMode::DefaultBranch {
        return Err(err(format!(
            "alias {} cannot define both `ref` and `track`",
            alias
        )));
    }

    lockfile.upsert(entry);
    Ok(())
}

fn parse_section_header(line: &str, line_number: usize) -> Result<String> {
    let inner = line
        .strip_prefix("[repos.")
        .and_then(|value| value.strip_suffix(']'))
        .ok_or_else(|| err(format!("invalid section header on line {}", line_number)))?;
    if !is_valid_alias(inner) {
        return Err(err(format!(
            "invalid alias in section header on line {}: {}",
            line_number, inner
        )));
    }
    Ok(inner.to_string())
}

fn parse_quoted_value(raw: &str, line_number: usize) -> Result<String> {
    let Some(inner) = raw
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    else {
        return Err(err(format!(
            "invalid value on line {}: expected quoted string",
            line_number
        )));
    };
    Ok(inner.replace("\\\"", "\""))
}

fn escape(value: &str) -> String {
    value.replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::{LockEntry, Lockfile, TrackMode};

    #[test]
    fn parses_default_and_ref_entries() {
        let lockfile = Lockfile::parse(
            r#"[repos.grepo]
url = "git@github.com:tomrford/grepo.git"
track = "default"
commit = "abc"

[repos.mint]
url = "git@github.com:tomrford/mint.git"
ref = "main"
commit = "def"
"#,
        )
        .unwrap();

        assert_eq!(
            lockfile.get("grepo"),
            Some(&LockEntry {
                alias: "grepo".into(),
                url: "git@github.com:tomrford/grepo.git".into(),
                ref_name: None,
                track: TrackMode::DefaultBranch,
                commit: Some("abc".into()),
            })
        );
        assert_eq!(
            lockfile.get("mint").unwrap().ref_name.as_deref(),
            Some("main")
        );
    }

    #[test]
    fn renders_canonical_format() {
        let mut lockfile = Lockfile::empty();
        let mut entry = LockEntry::new("mint".into(), "git@github.com:tomrford/mint.git".into());
        entry.ref_name = Some("main".into());
        entry.commit = Some("abc".into());
        lockfile.upsert(entry);

        assert_eq!(
            lockfile.render(),
            "[repos.mint]\nurl = \"git@github.com:tomrford/mint.git\"\nref = \"main\"\ncommit = \"abc\"\n\n"
        );
    }
}
