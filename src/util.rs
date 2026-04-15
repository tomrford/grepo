use std::ffi::{OsStr, OsString};
use std::fmt::{self, Display, Formatter};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, SystemTime};

use thiserror::Error;

pub type UtilResult<T> = std::result::Result<T, UtilError>;
pub type CommandResult<T> = std::result::Result<T, CommandError>;

#[derive(Debug, Error)]
pub enum UtilError {
    #[error("failed to determine current directory: {0}")]
    CurrentDir(#[source] std::io::Error),

    #[error("failed to determine OS cache directory")]
    MissingCacheRoot,

    #[error("failed to determine OS state directory")]
    MissingStateRoot,

    #[error("{path} has no parent directory")]
    MissingParent { path: PathBuf },

    #[error("failed to create directory {path}: {source}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to write temporary file {path}: {source}")]
    WriteTempFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to rename {from} to {to}: {source}")]
    Rename {
        from: PathBuf,
        to: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Error)]
pub enum CommandError {
    #[error("failed to spawn {cmd}: {source}")]
    Spawn {
        cmd: String,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to open stdin for {cmd}")]
    MissingStdin { cmd: String },

    #[error("failed to write stdin for {cmd}: {source}")]
    WriteStdin {
        cmd: String,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to wait for {cmd}: {source}")]
    Wait {
        cmd: String,
        #[source]
        source: std::io::Error,
    },

    #[error("{cmd} failed{detail}")]
    Failed { cmd: String, detail: String },
}

pub fn current_dir() -> UtilResult<PathBuf> {
    std::env::current_dir().map_err(UtilError::CurrentDir)
}

pub fn cache_root() -> UtilResult<PathBuf> {
    dirs::cache_dir()
        .map(|path| path.join("grepo"))
        .ok_or(UtilError::MissingCacheRoot)
}

pub fn state_root() -> UtilResult<PathBuf> {
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .map(|path| path.join("grepo"))
        .ok_or(UtilError::MissingStateRoot)
}

pub fn ensure_dir(path: &Path) -> UtilResult<()> {
    fs::create_dir_all(path).map_err(|source| UtilError::CreateDir {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(())
}

pub fn write_atomic(path: &Path, contents: &str) -> UtilResult<()> {
    let parent = path.parent().ok_or_else(|| UtilError::MissingParent {
        path: path.to_path_buf(),
    })?;
    ensure_dir(parent)?;
    let temp = unique_path(parent, ".grepo-write");
    fs::write(&temp, contents).map_err(|source| UtilError::WriteTempFile {
        path: temp.clone(),
        source,
    })?;
    fs::rename(&temp, path).map_err(|source| UtilError::Rename {
        from: temp,
        to: path.to_path_buf(),
        source,
    })?;
    Ok(())
}

pub fn unique_path(parent: &Path, prefix: &str) -> PathBuf {
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_nanos();
    parent.join(format!("{prefix}-{pid}-{nanos}"))
}

pub fn run_command(
    program: &OsStr,
    args: &[OsString],
    cwd: Option<&Path>,
    stdin_data: Option<&[u8]>,
) -> CommandResult<CommandOutput> {
    let cmd = format_command(program, args);
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

    let mut child = command.spawn().map_err(|source| CommandError::Spawn {
        cmd: cmd.clone(),
        source,
    })?;

    if let Some(input) = stdin_data {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| CommandError::MissingStdin { cmd: cmd.clone() })?;
        stdin
            .write_all(input)
            .map_err(|source| CommandError::WriteStdin {
                cmd: cmd.clone(),
                source,
            })?;
    }

    let output = child
        .wait_with_output()
        .map_err(|source| CommandError::Wait { cmd, source })?;

    Ok(CommandOutput {
        status: output.status,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

pub struct CommandOutput {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutput {
    pub fn success(self, program: &OsStr, args: &[OsString]) -> CommandResult<Self> {
        if self.status.success() {
            return Ok(self);
        }

        let cmd = format_command(program, args);
        let stderr = self.stderr.trim();
        let detail = if stderr.is_empty() {
            format!(" with status {}", ExitStatusDisplay(self.status))
        } else {
            format!(": {stderr}")
        };
        Err(CommandError::Failed { cmd, detail })
    }
}

struct ExitStatusDisplay(ExitStatus);

impl Display for ExitStatusDisplay {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}

fn format_command(program: &OsStr, args: &[OsString]) -> String {
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

pub fn is_valid_alias(alias: &str) -> bool {
    !alias.is_empty()
        && !alias.starts_with('.')
        && alias
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
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
