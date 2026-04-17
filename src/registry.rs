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
}

#[derive(Debug, Deserialize)]
struct NpmPackage {
    #[serde(rename = "dist-tags")]
    dist_tags: NpmDistTags,
}

#[derive(Debug, Deserialize)]
struct NpmDistTags {
    latest: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum NpmRepository {
    String(String),
    Object {
        #[serde(rename = "type")]
        repo_type: Option<String>,
        url: String,
        #[serde(default)]
        directory: Option<String>,
    },
}

pub fn resolve_npm(agent: &ureq::Agent, name: &str, version: Option<&str>) -> Result<NpmResolved> {
    let encoded_name = encode_npm_package_name(name);
    let version = match version {
        Some(version) => version.to_string(),
        None => latest_npm_version(agent, name, &encoded_name)?,
    };
    let url = format!("https://registry.npmjs.org/{encoded_name}/{version}");
    let body: NpmVersion = get_json(agent, &url)?;
    let git_head = body.git_head.ok_or_else(|| {
        GrepoError::Registry(format!(
            "npm:{name}@{version} does not publish gitHead metadata; grepo cannot map this \
release to an exact source commit automatically. Use --url for the upstream repo if you want a raw git source"
        ))
    })?;
    let commit = normalise_commit(&git_head).ok_or_else(|| {
        GrepoError::Registry(format!(
            "npm:{name}@{version} gitHead is not a commit SHA: {git_head}"
        ))
    })?;
    let repository = body.repository.ok_or_else(|| {
        GrepoError::Registry(format!(
            "npm:{name}@{version} does not publish repository metadata; use --url for the upstream repo if you want a raw git source"
        ))
    })?;
    let (raw_url, subdir) = match repository {
        NpmRepository::String(url) => (url, None),
        NpmRepository::Object {
            repo_type,
            url,
            directory,
        } => {
            if let Some(repo_type) = repo_type
                && repo_type != "git"
            {
                return Err(GrepoError::Registry(format!(
                    "npm:{name}@{version} repository type {repo_type:?} is not supported; use --url for the upstream repo"
                )));
            }
            (url, directory)
        }
    };
    let git_url = normalise_git_url(&raw_url).map_err(|error| {
        GrepoError::Registry(format!(
            "npm:{name}@{version} repository {raw_url:?} is not a supported git source: {error}. Use --url for the upstream repo"
        ))
    })?;
    Ok(NpmResolved {
        url: git_url,
        subdir,
        commit,
        version: version.to_string(),
    })
}

fn latest_npm_version(agent: &ureq::Agent, name: &str, encoded_name: &str) -> Result<String> {
    let url = format!("https://registry.npmjs.org/{encoded_name}");
    let body: NpmPackage = get_json(agent, &url)?;
    body.dist_tags
        .latest
        .ok_or_else(|| GrepoError::Registry(format!("npm:{name} has no dist-tags.latest version")))
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

fn normalise_git_url(raw: &str) -> Result<String> {
    let url = raw.trim();
    let url = strip_fragment(url);

    if let Some(repo) = url.strip_prefix("github:") {
        return expand_host_shorthand("https://github.com", repo);
    }
    if let Some(repo) = url.strip_prefix("gitlab:") {
        return expand_host_shorthand("https://gitlab.com", repo);
    }
    if let Some(repo) = url.strip_prefix("bitbucket:") {
        return expand_host_shorthand("https://bitbucket.org", repo);
    }
    if url.starts_with("gist:") {
        return Err(GrepoError::InvalidSource(
            "gist repositories are not supported".into(),
        ));
    }
    if looks_like_github_shorthand(url) {
        return expand_host_shorthand("https://github.com", url);
    }

    let url = url.strip_prefix("git+").unwrap_or(url);
    if is_supported_git_url(url) {
        return Ok(ensure_git_suffix(url));
    }

    Err(GrepoError::InvalidSource(format!(
        "unsupported repository URL {raw:?}"
    )))
}

fn strip_fragment(url: &str) -> &str {
    url.split('#').next().unwrap_or(url)
}

fn looks_like_github_shorthand(value: &str) -> bool {
    let Some((owner, repo)) = value.split_once('/') else {
        return false;
    };
    !owner.is_empty()
        && !repo.is_empty()
        && !value.contains("://")
        && !value.contains('@')
        && !owner.contains(':')
        && !repo.contains('/')
}

fn expand_host_shorthand(base: &str, repo: &str) -> Result<String> {
    if !looks_like_github_shorthand(repo) {
        return Err(GrepoError::InvalidSource(format!(
            "unsupported repository shorthand {repo:?}"
        )));
    }
    Ok(ensure_git_suffix(&format!("{base}/{repo}")))
}

fn is_supported_git_url(url: &str) -> bool {
    url.starts_with("https://")
        || url.starts_with("http://")
        || url.starts_with("ssh://")
        || url.starts_with("git://")
        || url.starts_with("git@")
}

fn ensure_git_suffix(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    if trimmed.ends_with(".git") {
        trimmed.to_string()
    } else {
        format!("{trimmed}.git")
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
struct CargoCrateRegistryResponse {
    #[serde(rename = "crate")]
    krate: CargoRegistryCrate,
    versions: Vec<CargoVersion>,
}

#[derive(Debug, Deserialize)]
struct CargoRegistryCrate {
    #[serde(rename = "max_stable_version")]
    max_stable_version: String,
}

#[derive(Debug, Deserialize)]
struct CargoVersion {
    num: String,
    #[serde(rename = "dl_path")]
    dl_path: String,
    #[serde(rename = "checksum")]
    checksum: String,
    #[serde(default)]
    yanked: bool,
}

pub fn resolve_cargo(
    agent: &ureq::Agent,
    name: &str,
    version: Option<&str>,
) -> Result<CargoResolved> {
    match version {
        Some(version) => resolve_exact_cargo(agent, name, version),
        None => resolve_latest_cargo(agent, name),
    }
}

fn resolve_exact_cargo(agent: &ureq::Agent, name: &str, version: &str) -> Result<CargoResolved> {
    let url = format!("https://crates.io/api/v1/crates/{name}/{version}");
    let body: CargoRegistryResponse = get_json(agent, &url)?;
    cargo_resolved_from_version(name, &body.version)
}

fn resolve_latest_cargo(agent: &ureq::Agent, name: &str) -> Result<CargoResolved> {
    let url = format!("https://crates.io/api/v1/crates/{name}");
    let body: CargoCrateRegistryResponse = get_json(agent, &url)?;
    let version = select_latest_cargo_version(&body).ok_or_else(|| {
        GrepoError::Registry(format!("cargo:{name} has no stable non-yanked release"))
    })?;
    cargo_resolved_from_version(name, version)
}

fn select_latest_cargo_version(body: &CargoCrateRegistryResponse) -> Option<&CargoVersion> {
    if !body.krate.max_stable_version.is_empty()
        && let Some(version) = body
            .versions
            .iter()
            .find(|version| version.num == body.krate.max_stable_version && !version.yanked)
    {
        return Some(version);
    }
    body.versions.iter().find(|version| !version.yanked)
}

fn cargo_resolved_from_version(name: &str, version: &CargoVersion) -> Result<CargoResolved> {
    let download_url = if version.dl_path.starts_with("http") {
        version.dl_path.clone()
    } else {
        format!("https://crates.io{}", version.dl_path)
    };
    let sha256 = version.checksum.to_ascii_lowercase();
    if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(GrepoError::Registry(format!(
            "cargo:{name}@{} returned invalid checksum: {}",
            version.num, version.checksum
        )));
    }
    Ok(CargoResolved {
        version: version.num.clone(),
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
            normalise_git_url("git+https://github.com/facebook/react.git").unwrap(),
            "https://github.com/facebook/react.git"
        );
        assert_eq!(
            normalise_git_url("https://github.com/facebook/react").unwrap(),
            "https://github.com/facebook/react.git"
        );
    }

    #[test]
    fn normalise_git_url_supports_github_shorthand() {
        assert_eq!(
            normalise_git_url("colinhacks/zod").unwrap(),
            "https://github.com/colinhacks/zod.git"
        );
        assert_eq!(
            normalise_git_url("github:colinhacks/zod").unwrap(),
            "https://github.com/colinhacks/zod.git"
        );
    }

    #[test]
    fn normalise_git_url_supports_other_host_shorthands() {
        assert_eq!(
            normalise_git_url("gitlab:honojs/middleware").unwrap(),
            "https://gitlab.com/honojs/middleware.git"
        );
        assert_eq!(
            normalise_git_url("bitbucket:user/repo").unwrap(),
            "https://bitbucket.org/user/repo.git"
        );
    }

    #[test]
    fn normalise_git_url_strips_fragments_and_trailing_slash() {
        assert_eq!(
            normalise_git_url("https://github.com/npm/cli/#readme").unwrap(),
            "https://github.com/npm/cli.git"
        );
    }

    #[test]
    fn normalise_git_url_rejects_unsupported_shorthands() {
        assert!(normalise_git_url("gist:11081aaa281").is_err());
        assert!(normalise_git_url("not a repo").is_err());
    }

    #[test]
    fn latest_npm_version_uses_dist_tag() {
        let body: NpmPackage = serde_json::from_str(
            r#"{
                "dist-tags": {
                    "latest": "19.2.5",
                    "next": "19.3.0-canary"
                }
            }"#,
        )
        .unwrap();

        assert_eq!(body.dist_tags.latest.as_deref(), Some("19.2.5"));
    }

    #[test]
    fn select_latest_cargo_version_prefers_max_stable_version() {
        let body: CargoCrateRegistryResponse = serde_json::from_str(
            r#"{
                "crate": {
                    "max_stable_version": "1.0.228"
                },
                "versions": [
                    {
                        "num": "1.0.228",
                        "dl_path": "/api/v1/crates/serde/1.0.228/download",
                        "checksum": "9a8e94ea7f378bd32cbbd37198a4a91436180c5bb472411e48b5ec2e2124ae9e",
                        "yanked": false
                    },
                    {
                        "num": "1.0.229-alpha.1",
                        "dl_path": "/api/v1/crates/serde/1.0.229-alpha.1/download",
                        "checksum": "80ece43fc6fbed4eb5392ab50c07334d3e577cbf40997ee896fe7af40bba4245",
                        "yanked": false
                    }
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(
            select_latest_cargo_version(&body).map(|version| version.num.as_str()),
            Some("1.0.228")
        );
    }
}
