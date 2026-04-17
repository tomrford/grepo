use serde::Deserialize;

use super::get_json;
use crate::error::{GrepoError, Result};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CargoResolved {
    pub version: String,
    pub download_url: String,
    pub sha256: String,
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
