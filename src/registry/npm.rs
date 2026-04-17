use serde::Deserialize;

use super::get_json;
use crate::error::{GrepoError, Result};
use crate::git::validate_commit_oid;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NpmResolved {
    pub url: String,
    pub subdir: Option<String>,
    pub commit: String,
    pub version: String,
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
    validate_commit_oid(trimmed).ok()?;
    Some(trimmed.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
