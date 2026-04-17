use std::time::Duration;

use serde::Deserialize;

use crate::error::{GrepoError, Result};

const USER_AGENT: &str = concat!(
    "grepo/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/tomrford/grepo)"
);

pub fn http_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(60))
        .user_agent(USER_AGENT)
        .build()
}

pub fn get_json<T: for<'de> Deserialize<'de>>(agent: &ureq::Agent, url: &str) -> Result<T> {
    let response = agent.get(url).call().map_err(|e| match e {
        ureq::Error::Status(code, resp) => GrepoError::Registry(format!(
            "GET {url} returned {code}: {}",
            resp.into_string().unwrap_or_default()
        )),
        other => GrepoError::Registry(format!("GET {url} failed: {other}")),
    })?;
    response
        .into_json::<T>()
        .map_err(|e| GrepoError::Registry(format!("GET {url} returned invalid JSON: {e}")))
}

pub fn get_bytes(agent: &ureq::Agent, url: &str) -> Result<Vec<u8>> {
    let response = agent.get(url).call().map_err(|e| match e {
        ureq::Error::Status(code, resp) => GrepoError::Registry(format!(
            "GET {url} returned {code}: {}",
            resp.into_string().unwrap_or_default()
        )),
        other => GrepoError::Registry(format!("GET {url} failed: {other}")),
    })?;
    let mut buf = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut buf)
        .map_err(|e| GrepoError::Registry(format!("GET {url} body read failed: {e}")))?;
    Ok(buf)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceSpec {
    Npm {
        spec: String,
        name: String,
        version: Option<String>,
    },
    Cargo {
        spec: String,
        name: String,
        version: Option<String>,
    },
}

impl SourceSpec {
    pub fn to_source_string(&self) -> Option<String> {
        match self {
            Self::Npm { spec, .. } => Some(format!("npm:{spec}")),
            Self::Cargo { spec, .. } => Some(format!("cargo:{spec}")),
        }
    }

    pub fn parse_lock_source(raw: &str) -> Result<Self> {
        let (scheme, rest) = raw
            .split_once(':')
            .ok_or_else(|| GrepoError::InvalidSource(raw.to_string()))?;
        match scheme {
            "npm" => Self::parse_npm(rest),
            "cargo" => Self::parse_cargo(rest),
            other => Err(GrepoError::InvalidSource(format!(
                "unknown source scheme \"{other}\""
            ))),
        }
    }

    pub fn parse_npm(spec: &str) -> Result<Self> {
        let (name, version) =
            split_npm_spec(spec).ok_or_else(|| GrepoError::InvalidSource(format!("npm:{spec}")))?;
        require_exact_version(version.as_deref())?;
        Ok(Self::Npm {
            spec: spec.to_string(),
            name,
            version,
        })
    }

    pub fn parse_cargo(spec: &str) -> Result<Self> {
        let (name, version) = split_at_version(spec)
            .ok_or_else(|| GrepoError::InvalidSource(format!("cargo:{spec}")))?;
        require_exact_version(version.as_deref())?;
        Ok(Self::Cargo {
            spec: spec.to_string(),
            name,
            version,
        })
    }
}

fn require_exact_version(version: Option<&str>) -> Result<()> {
    let Some(version) = version else {
        return Ok(());
    };
    let trimmed = version.trim();
    if trimmed.is_empty() {
        return Err(GrepoError::InvalidSource("empty version".into()));
    }
    // v1: reject anything that isn't obviously an exact version.
    let first = trimmed.chars().next().unwrap();
    if matches!(first, '^' | '~' | '>' | '<' | '=') {
        return Err(GrepoError::InvalidSource(format!(
            "only exact versions are supported, got \"{trimmed}\""
        )));
    }
    if trimmed.contains(|c: char| c.is_whitespace() || c == ',' || c == '|') {
        return Err(GrepoError::InvalidSource(format!(
            "only exact versions are supported, got \"{trimmed}\""
        )));
    }
    if trimmed == "latest" || trimmed == "*" {
        return Err(GrepoError::InvalidSource(
            "dist-tags and wildcards are not supported; specify an exact version".into(),
        ));
    }
    Ok(())
}

fn split_npm_spec(spec: &str) -> Option<(String, Option<String>)> {
    if spec.is_empty() {
        return None;
    }
    if let Some(rest) = spec.strip_prefix('@') {
        let (scope_name, version) = match rest.find('@') {
            Some(idx) => {
                let (head, tail) = rest.split_at(idx);
                (head.to_string(), Some(tail[1..].to_string()))
            }
            None => (rest.to_string(), None),
        };
        if scope_name.is_empty() || !scope_name.contains('/') {
            return None;
        }
        Some((format!("@{scope_name}"), version))
    } else {
        split_at_version(spec)
    }
}

fn split_at_version(spec: &str) -> Option<(String, Option<String>)> {
    if spec.is_empty() {
        return None;
    }
    match spec.find('@') {
        Some(idx) => {
            let (head, tail) = spec.split_at(idx);
            if head.is_empty() {
                return None;
            }
            Some((head.to_string(), Some(tail[1..].to_string())))
        }
        None => Some((spec.to_string(), None)),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NpmResolved {
    pub url: String,
    pub subdir: Option<String>,
    pub commit: String,
    pub version: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CargoResolved {
    pub version: String,
    pub download_url: String,
    pub sha256: String,
}

#[derive(Debug, Deserialize)]
struct NpmVersion {
    #[serde(rename = "gitHead")]
    git_head: Option<String>,
    repository: Option<NpmRepository>,
    #[serde(rename = "dist-tags")]
    _dist_tags: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum NpmRepository {
    String(String),
    Object {
        url: String,
        #[serde(default)]
        directory: Option<String>,
    },
}

pub fn resolve_npm(agent: &ureq::Agent, name: &str, version: &str) -> Result<NpmResolved> {
    let encoded_name = encode_npm_package_name(name);
    let url = format!("https://registry.npmjs.org/{encoded_name}/{version}");
    let body: NpmVersion = get_json(agent, &url)?;
    let git_head = body.git_head.ok_or_else(|| {
        GrepoError::Registry(format!(
            "npm:{name}@{version} has no gitHead; cannot resolve an exact commit"
        ))
    })?;
    let commit = normalise_commit(&git_head).ok_or_else(|| {
        GrepoError::Registry(format!(
            "npm:{name}@{version} gitHead is not a commit SHA: {git_head}"
        ))
    })?;
    let repository = body.repository.ok_or_else(|| {
        GrepoError::Registry(format!(
            "npm:{name}@{version} has no repository; cannot resolve a git URL"
        ))
    })?;
    let (raw_url, subdir) = match repository {
        NpmRepository::String(url) => (url, None),
        NpmRepository::Object { url, directory } => (url, directory),
    };
    let git_url = normalise_git_url(&raw_url);
    Ok(NpmResolved {
        url: git_url,
        subdir,
        commit,
        version: version.to_string(),
    })
}

fn encode_npm_package_name(name: &str) -> String {
    if let Some(rest) = name.strip_prefix('@')
        && let Some(slash_idx) = rest.find('/')
    {
        let (scope, pkg) = rest.split_at(slash_idx);
        return format!("@{scope}%2F{}", &pkg[1..]);
    }
    name.to_string()
}

fn normalise_git_url(raw: &str) -> String {
    let url = raw.trim().strip_prefix("git+").unwrap_or(raw.trim());
    if url.ends_with(".git") {
        url.to_string()
    } else {
        format!("{url}.git")
    }
}

fn normalise_commit(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.len() != 40 && trimmed.len() != 64 {
        return None;
    }
    if !trimmed.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(trimmed.to_ascii_lowercase())
}

#[derive(Debug, Deserialize)]
struct CargoRegistryResponse {
    version: CargoVersion,
}

#[derive(Debug, Deserialize)]
struct CargoVersion {
    num: String,
    #[serde(rename = "dl_path")]
    dl_path: String,
    #[serde(rename = "checksum")]
    checksum: String,
}

pub fn resolve_cargo(agent: &ureq::Agent, name: &str, version: &str) -> Result<CargoResolved> {
    let url = format!("https://crates.io/api/v1/crates/{name}/{version}");
    let body: CargoRegistryResponse = get_json(agent, &url)?;
    let download_url = if body.version.dl_path.starts_with("http") {
        body.version.dl_path
    } else {
        format!("https://crates.io{}", body.version.dl_path)
    };
    let sha256 = body.version.checksum.to_ascii_lowercase();
    if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(GrepoError::Registry(format!(
            "cargo:{name}@{version} returned invalid checksum: {}",
            body.version.checksum
        )));
    }
    Ok(CargoResolved {
        version: body.version.num,
        download_url,
        sha256,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_npm_plain() {
        let spec = SourceSpec::parse_npm("react").unwrap();
        assert_eq!(
            spec,
            SourceSpec::Npm {
                spec: "react".into(),
                name: "react".into(),
                version: None,
            }
        );
    }

    #[test]
    fn parse_npm_with_version() {
        let spec = SourceSpec::parse_npm("react@18.2.0").unwrap();
        assert_eq!(
            spec,
            SourceSpec::Npm {
                spec: "react@18.2.0".into(),
                name: "react".into(),
                version: Some("18.2.0".into()),
            }
        );
    }

    #[test]
    fn parse_npm_scoped() {
        let spec = SourceSpec::parse_npm("@types/node@20.10.0").unwrap();
        assert_eq!(
            spec,
            SourceSpec::Npm {
                spec: "@types/node@20.10.0".into(),
                name: "@types/node".into(),
                version: Some("20.10.0".into()),
            }
        );
    }

    #[test]
    fn parse_npm_scoped_no_version() {
        let spec = SourceSpec::parse_npm("@types/node").unwrap();
        assert_eq!(
            spec,
            SourceSpec::Npm {
                spec: "@types/node".into(),
                name: "@types/node".into(),
                version: None,
            }
        );
    }

    #[test]
    fn parse_npm_rejects_range() {
        assert!(SourceSpec::parse_npm("react@^18.0.0").is_err());
        assert!(SourceSpec::parse_npm("react@latest").is_err());
    }

    #[test]
    fn parse_cargo_plain() {
        let spec = SourceSpec::parse_cargo("serde").unwrap();
        assert_eq!(
            spec,
            SourceSpec::Cargo {
                spec: "serde".into(),
                name: "serde".into(),
                version: None,
            }
        );
    }

    #[test]
    fn parse_cargo_with_version() {
        let spec = SourceSpec::parse_cargo("serde@1.0.197").unwrap();
        assert_eq!(
            spec,
            SourceSpec::Cargo {
                spec: "serde@1.0.197".into(),
                name: "serde".into(),
                version: Some("1.0.197".into()),
            }
        );
    }

    #[test]
    fn parse_lock_source_roundtrip() {
        let spec = SourceSpec::parse_lock_source("npm:react@18.2.0").unwrap();
        assert_eq!(spec.to_source_string().as_deref(), Some("npm:react@18.2.0"));
    }

    #[test]
    fn encode_npm_scoped_name_url_encodes_slash() {
        assert_eq!(encode_npm_package_name("react"), "react");
        assert_eq!(encode_npm_package_name("@types/node"), "@types%2Fnode");
    }

    #[test]
    fn normalise_git_url_strips_git_plus_and_keeps_suffix() {
        assert_eq!(
            normalise_git_url("git+https://github.com/facebook/react.git"),
            "https://github.com/facebook/react.git"
        );
        assert_eq!(
            normalise_git_url("https://github.com/facebook/react"),
            "https://github.com/facebook/react.git"
        );
    }
}
