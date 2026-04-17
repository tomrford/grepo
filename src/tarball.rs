use std::fs;
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use tar::Archive;

use crate::error::{GrepoError, Result};
use crate::util::{ensure_dir_mode, unique_path};

pub fn verify_sha256(bytes: &[u8], expected: &str) -> Result<()> {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let actual = hex_encode(&hasher.finalize());
    if actual != expected.to_ascii_lowercase() {
        return Err(GrepoError::Integrity(format!(
            "expected sha256 {expected}, got {actual}"
        )));
    }
    Ok(())
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

pub fn extract_crate_tarball(bytes: &[u8], target_dir: &Path) -> Result<()> {
    let parent = target_dir.parent().ok_or_else(|| {
        GrepoError::Io(format!(
            "snapshot path has no parent: {}",
            target_dir.display()
        ))
    })?;
    ensure_dir_mode(parent, 0o700)?;
    let temp_dir = unique_path(parent, ".grepo-tarball");
    fs::create_dir(&temp_dir)
        .map_err(|e| GrepoError::Io(format!("failed to create {}: {e}", temp_dir.display())))?;

    match unpack_into(bytes, &temp_dir) {
        Ok(top_level) => {
            let final_src = temp_dir.join(&top_level);
            fs::rename(&final_src, target_dir).map_err(|e| {
                GrepoError::Io(format!(
                    "failed to move snapshot into place {} -> {}: {e}",
                    final_src.display(),
                    target_dir.display()
                ))
            })?;
            let _ = fs::remove_dir_all(&temp_dir);
            Ok(())
        }
        Err(error) => {
            let _ = fs::remove_dir_all(&temp_dir);
            Err(error)
        }
    }
}

fn unpack_into(bytes: &[u8], dest: &Path) -> Result<PathBuf> {
    let decoder = GzDecoder::new(bytes);
    let mut archive = Archive::new(decoder);
    archive.set_preserve_permissions(false);
    let entries = archive
        .entries()
        .map_err(|e| GrepoError::Io(format!("failed to read tarball entries: {e}")))?;

    let mut top_level: Option<PathBuf> = None;
    for entry in entries {
        let mut entry =
            entry.map_err(|e| GrepoError::Io(format!("failed to read tarball entry: {e}")))?;
        let path = entry
            .path()
            .map_err(|e| GrepoError::Io(format!("tarball entry has invalid path: {e}")))?
            .into_owned();
        ensure_safe_relative_path(&path)?;
        let root = path
            .components()
            .next()
            .map(|c| PathBuf::from(c.as_os_str()))
            .ok_or_else(|| GrepoError::Io("tarball has empty entry path".to_string()))?;
        match &top_level {
            Some(existing) if existing != &root => {
                return Err(GrepoError::Io(format!(
                    "tarball has multiple top-level directories: {} and {}",
                    existing.display(),
                    root.display()
                )));
            }
            Some(_) => {}
            None => top_level = Some(root.clone()),
        }

        let full = dest.join(&path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                GrepoError::Io(format!("failed to create {}: {e}", parent.display()))
            })?;
        }
        entry
            .unpack(&full)
            .map_err(|e| GrepoError::Io(format!("failed to extract {}: {e}", full.display())))?;
    }

    top_level.ok_or_else(|| GrepoError::Io("tarball was empty".into()))
}

fn ensure_safe_relative_path(path: &Path) -> Result<()> {
    use std::path::Component;
    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            Component::CurDir => {}
            _ => {
                return Err(GrepoError::Io(format!(
                    "tarball entry contains disallowed path component: {}",
                    path.display()
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use flate2::Compression;
    use flate2::write::GzEncoder;
    use tar::Builder;

    use super::*;
    use crate::util::unique_path;

    #[test]
    fn sha256_check_passes_for_known_value() {
        let hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        verify_sha256(b"", hash).unwrap();
    }

    #[test]
    fn sha256_check_rejects_mismatch() {
        let err = verify_sha256(b"x", "00").unwrap_err();
        assert!(format!("{err}").contains("expected sha256 00"));
    }

    #[test]
    fn rejects_absolute_path() {
        let err = ensure_safe_relative_path(Path::new("/etc/passwd")).unwrap_err();
        assert!(format!("{err}").contains("disallowed"));
    }

    #[test]
    fn rejects_parent_escape() {
        let err = ensure_safe_relative_path(Path::new("../foo")).unwrap_err();
        assert!(format!("{err}").contains("disallowed"));
    }

    #[test]
    fn extract_crate_tarball_handles_file_entries_without_explicit_dirs() {
        let bytes = crate_tarball_bytes();
        let temp_root = unique_path(&std::env::temp_dir(), "grepo-tarball-test");
        fs::create_dir(&temp_root).unwrap();
        let target = temp_root.join("serde");

        extract_crate_tarball(&bytes, &target).unwrap();

        assert_eq!(
            fs::read_to_string(target.join("Cargo.toml")).unwrap(),
            "name = \"serde\"\n"
        );
        assert_eq!(
            fs::read_to_string(target.join(".cargo_vcs_info.json")).unwrap(),
            "{}\n"
        );

        let _ = fs::remove_dir_all(&temp_root);
    }

    fn crate_tarball_bytes() -> Vec<u8> {
        let encoder = GzEncoder::new(Vec::new(), Compression::default());
        let mut builder = Builder::new(encoder);

        let mut cargo_header = tar::Header::new_gnu();
        let cargo_bytes = b"name = \"serde\"\n";
        cargo_header.set_size(cargo_bytes.len() as u64);
        cargo_header.set_mode(0o644);
        cargo_header.set_cksum();
        builder
            .append_data(
                &mut cargo_header,
                "serde-1.0.228/Cargo.toml",
                Cursor::new(cargo_bytes),
            )
            .unwrap();

        let mut vcs_header = tar::Header::new_gnu();
        let vcs_bytes = b"{}\n";
        vcs_header.set_size(vcs_bytes.len() as u64);
        vcs_header.set_mode(0o644);
        vcs_header.set_cksum();
        builder
            .append_data(
                &mut vcs_header,
                "serde-1.0.228/.cargo_vcs_info.json",
                Cursor::new(vcs_bytes),
            )
            .unwrap();

        let encoder = builder.into_inner().unwrap();
        encoder.finish().unwrap()
    }
}
