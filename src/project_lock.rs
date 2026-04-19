use std::fs;
use std::path::Path;

use cargo_lock::Lockfile as CargoLockfile;
use serde::Deserialize;

use crate::error::{GrepoError, Result};
use crate::registry::SourceSpec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectLockKind {
    Cargo,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ResolvedDependencyVersion {
    pub(crate) name: String,
    pub(crate) version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectDependencySnapshot {
    pub(crate) kind: ProjectLockKind,
    pub(crate) direct_dependencies: Vec<ResolvedDependencyVersion>,
}

impl ProjectDependencySnapshot {
    pub(crate) fn version_for(&self, source: &str) -> Option<&str> {
        match (self.kind, SourceSpec::parse_lock_source(source).ok()?) {
            (ProjectLockKind::Cargo, SourceSpec::Cargo { name, .. }) => self
                .direct_dependencies
                .iter()
                .find(|dep| dep.name == name)
                .map(|dep| dep.version.as_str()),
            _ => None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct CargoManifest {
    package: Option<CargoPackage>,
}

#[derive(Debug, Deserialize)]
struct CargoPackage {
    name: String,
    version: String,
}

pub(crate) fn parse_cargo_project_lock(root_dir: &Path) -> Result<ProjectDependencySnapshot> {
    let manifest_path = root_dir.join("Cargo.toml");
    let manifest_contents = fs::read_to_string(&manifest_path).map_err(|error| {
        GrepoError::ProjectLock(format!("{}: {error}", manifest_path.display()))
    })?;
    let manifest: CargoManifest = toml::from_str(&manifest_contents).map_err(|error| {
        GrepoError::ProjectLock(format!("{}: {error}", manifest_path.display()))
    })?;
    let package = manifest.package.ok_or_else(|| {
        GrepoError::ProjectLock(format!(
            "{}: virtual workspaces are not supported yet",
            manifest_path.display()
        ))
    })?;

    let lock_path = root_dir.join("Cargo.lock");
    let lockfile = CargoLockfile::load(&lock_path)
        .map_err(|error| GrepoError::ProjectLock(format!("{}: {error}", lock_path.display())))?;
    let root_package = lockfile
        .packages
        .iter()
        .find(|pkg| pkg.name.as_str() == package.name && pkg.version.to_string() == package.version)
        .ok_or_else(|| {
            GrepoError::ProjectLock(format!(
                "{}: root package {} {} was not found in Cargo.lock",
                lock_path.display(),
                package.name,
                package.version
            ))
        })?;

    let mut direct_dependencies = root_package
        .dependencies
        .iter()
        .map(|dep| ResolvedDependencyVersion {
            name: dep.name.as_str().to_string(),
            version: dep.version.to_string(),
        })
        .collect::<Vec<_>>();
    direct_dependencies.sort();
    direct_dependencies.dedup();

    Ok(ProjectDependencySnapshot {
        kind: ProjectLockKind::Cargo,
        direct_dependencies,
    })
}

pub(crate) fn parse(path: &Path) -> Result<ProjectDependencySnapshot> {
    match path.file_name().and_then(|name| name.to_str()) {
        Some("Cargo.lock") => {
            let root_dir = path.parent().ok_or_else(|| {
                GrepoError::ProjectLock(format!(
                    "{}: lockfile has no parent directory",
                    path.display()
                ))
            })?;
            parse_cargo_project_lock(root_dir)
        }
        Some(name) => Err(GrepoError::ProjectLock(format!(
            "{}: unsupported project lockfile {name}",
            path.display()
        ))),
        None => Err(GrepoError::ProjectLock(format!(
            "{}: invalid project lockfile path",
            path.display()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::{ProjectLockKind, parse, parse_cargo_project_lock};

    #[test]
    fn parses_grepo_root_cargo_lock() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let snapshot = parse_cargo_project_lock(root).unwrap();

        assert_eq!(snapshot.kind, ProjectLockKind::Cargo);
        assert!(
            snapshot
                .direct_dependencies
                .iter()
                .any(|dep| { dep.name == "clap" && dep.version.starts_with('4') })
        );
        assert!(
            snapshot
                .direct_dependencies
                .iter()
                .any(|dep| { dep.name == "thiserror" && dep.version.starts_with('2') })
        );
    }

    #[test]
    fn detects_cargo_lock_by_path() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.lock");
        let snapshot = parse(&path).unwrap();

        assert_eq!(snapshot.kind, ProjectLockKind::Cargo);
        assert_eq!(snapshot.version_for("cargo:clap"), Some("4.6.1"));
    }
}
