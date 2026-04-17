use std::time::Duration;

use serde::Deserialize;

use crate::error::{GrepoError, Result};

mod cargo;
mod npm;

pub use cargo::{CargoResolved, resolve_cargo};
pub use npm::{NpmResolved, resolve_npm};

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

pub(crate) fn get_json<T: for<'de> Deserialize<'de>>(agent: &ureq::Agent, url: &str) -> Result<T> {
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
        name: String,
        version: Option<String>,
    },
    Cargo {
        name: String,
        version: Option<String>,
    },
}

impl SourceSpec {
    pub fn to_source_string(&self) -> Option<String> {
        match self {
            Self::Npm { name, version } => {
                Some(format!("npm:{}", format_spec(name, version.as_deref())))
            }
            Self::Cargo { name, version } => {
                Some(format!("cargo:{}", format_spec(name, version.as_deref())))
            }
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
        Ok(Self::Npm { name, version })
    }

    pub fn parse_cargo(spec: &str) -> Result<Self> {
        let (name, version) = split_at_version(spec)
            .ok_or_else(|| GrepoError::InvalidSource(format!("cargo:{spec}")))?;
        require_exact_version(version.as_deref())?;
        Ok(Self::Cargo { name, version })
    }
}

fn format_spec(name: &str, version: Option<&str>) -> String {
    match version {
        Some(v) => format!("{name}@{v}"),
        None => name.to_string(),
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
    // Only exact versions are supported; reject range/wildcard/tag syntax.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_npm_plain() {
        let spec = SourceSpec::parse_npm("react").unwrap();
        assert_eq!(
            spec,
            SourceSpec::Npm {
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
}
