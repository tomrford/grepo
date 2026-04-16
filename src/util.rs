use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, SystemTime};

use crate::error::{GrepoError, Result};

pub fn current_dir() -> Result<PathBuf> {
    std::env::current_dir()
        .map_err(|e| GrepoError::Io(format!("failed to determine current directory: {e}")))
}

pub fn cache_root() -> Result<PathBuf> {
    dirs::cache_dir()
        .map(|path| path.join("grepo"))
        .ok_or_else(|| GrepoError::Io("failed to determine OS cache directory".into()))
}

pub fn state_root() -> Result<PathBuf> {
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .map(|path| path.join("grepo"))
        .ok_or_else(|| GrepoError::Io("failed to determine OS state directory".into()))
}

pub fn ensure_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|e| {
        GrepoError::Io(format!(
            "failed to create directory {}: {e}",
            path.display()
        ))
    })
}

pub fn ensure_dir_mode(path: &Path, mode: u32) -> Result<()> {
    ensure_dir(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|e| {
        GrepoError::Io(format!(
            "failed to set permissions on {}: {e}",
            path.display()
        ))
    })
}

pub fn write_atomic(path: &Path, contents: &str) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| GrepoError::Io(format!("{} has no parent directory", path.display())))?;
    ensure_dir(parent)?;
    let temp = unique_path(parent, ".grepo-write");
    fs::write(&temp, contents).map_err(|e| {
        GrepoError::Io(format!(
            "failed to write temporary file {}: {e}",
            temp.display()
        ))
    })?;
    fs::rename(&temp, path).map_err(|e| {
        GrepoError::Io(format!(
            "failed to rename {} to {}: {e}",
            temp.display(),
            path.display()
        ))
    })
}

pub fn unique_path(parent: &Path, prefix: &str) -> PathBuf {
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_nanos();
    parent.join(format!("{prefix}-{pid}-{nanos}"))
}

pub fn is_valid_alias(alias: &str) -> bool {
    !alias.is_empty()
        && !alias.starts_with('.')
        && alias
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
}

pub struct CommandOutput {
    pub cmd: String,
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutput {
    pub fn check(self) -> Result<Self> {
        if self.status.success() {
            return Ok(self);
        }
        let stderr = self.stderr.trim();
        let detail = if stderr.is_empty() {
            format!("status {}", self.status)
        } else {
            stderr.to_string()
        };
        Err(GrepoError::Command(format!(
            "{} failed: {detail}",
            self.cmd
        )))
    }
}

pub fn run_command(
    program: &OsStr,
    args: &[OsString],
    cwd: Option<&Path>,
    stdin_data: Option<&[u8]>,
) -> Result<CommandOutput> {
    let cmd = render_cmd(program, args);
    let mut command = Command::new(program);
    command.args(args);
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    if stdin_data.is_some() {
        command.stdin(Stdio::piped());
    }
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .map_err(|e| GrepoError::Command(format!("failed to spawn {cmd}: {e}")))?;

    if let Some(input) = stdin_data {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| GrepoError::Command(format!("failed to open stdin for {cmd}")))?;
        stdin
            .write_all(input)
            .map_err(|e| GrepoError::Command(format!("failed to write stdin for {cmd}: {e}")))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|e| GrepoError::Command(format!("failed to wait for {cmd}: {e}")))?;

    Ok(CommandOutput {
        cmd,
        status: output.status,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn render_cmd(program: &OsStr, args: &[OsString]) -> String {
    let mut rendered = program.to_string_lossy().into_owned();
    for arg in args {
        rendered.push(' ');
        rendered.push_str(&shellish(arg));
    }
    rendered
}

fn shell_escape(value: &str) -> String {
    let escaped = value.replace('\'', "'\"'\"'");
    format!("'{escaped}'")
}

fn shellish(value: &OsStr) -> String {
    let text = value.to_string_lossy();
    if text
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || "-_./:=+".contains(ch))
    {
        text.into_owned()
    } else {
        shell_escape(&text)
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::{OsStr, OsString};

    use super::render_cmd;

    #[test]
    fn render_cmd_quotes_args_with_spaces() {
        let rendered = render_cmd(
            OsStr::new("git"),
            &[
                OsString::from("fetch"),
                OsString::from("/tmp/my repo.git"),
                OsString::from("refs/heads/main"),
            ],
        );

        assert_eq!(rendered, "git fetch '/tmp/my repo.git' refs/heads/main");
    }

    #[test]
    fn render_cmd_escapes_single_quotes() {
        let rendered = render_cmd(OsStr::new("git"), &[OsString::from("a'b")]);

        assert_eq!(rendered, "git 'a'\"'\"'b'");
    }
}
