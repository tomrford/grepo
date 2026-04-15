use std::ffi::{OsStr, OsString};
use std::fmt::{self, Display, Formatter};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, SystemTime};

pub type Result<T> = std::result::Result<T, GrepoError>;

#[derive(Debug, Clone)]
pub struct GrepoError {
    message: String,
}

impl GrepoError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl Display for GrepoError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for GrepoError {}

impl From<std::io::Error> for GrepoError {
    fn from(error: std::io::Error) -> Self {
        Self::new(error.to_string())
    }
}

pub fn err(message: impl Into<String>) -> GrepoError {
    GrepoError::new(message)
}

pub fn current_dir() -> Result<PathBuf> {
    std::env::current_dir().map_err(Into::into)
}

pub fn cache_root() -> Result<PathBuf> {
    dirs::cache_dir()
        .map(|path| path.join("grepo"))
        .ok_or_else(|| err("failed to determine OS cache directory"))
}

pub fn state_root() -> Result<PathBuf> {
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .map(|path| path.join("grepo"))
        .ok_or_else(|| err("failed to determine OS state directory"))
}

pub fn ensure_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    Ok(())
}

pub fn write_atomic(path: &Path, contents: &str) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| err(format!("{} has no parent directory", path.display())))?;
    ensure_dir(parent)?;
    let temp = unique_path(parent, ".grepo-write");
    fs::write(&temp, contents)?;
    fs::rename(temp, path)?;
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

fn format_command(program: &OsStr, args: &[OsString]) -> String {
    let mut rendered = program.to_string_lossy().into_owned();
    for arg in args {
        rendered.push(' ');
        rendered.push_str(&shellish(arg));
    }
    rendered
}

pub fn run_command(
    program: &OsStr,
    args: &[OsString],
    cwd: Option<&Path>,
    stdin_data: Option<&[u8]>,
) -> Result<CommandOutput> {
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

    let mut child = command.spawn().map_err(|error| {
        err(format!(
            "failed to spawn {}: {error}",
            format_command(program, args)
        ))
    })?;

    if let Some(input) = stdin_data {
        use std::io::Write;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| err("failed to open stdin for child process"))?;
        stdin.write_all(input)?;
    }

    let output = child.wait_with_output().map_err(|error| {
        err(format!(
            "failed to wait for {}: {error}",
            format_command(program, args)
        ))
    })?;

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
    pub fn success(self, program: &OsStr, args: &[OsString]) -> Result<Self> {
        if self.status.success() {
            return Ok(self);
        }

        let stderr = self.stderr.trim();
        let message = if stderr.is_empty() {
            format!(
                "{} exited with status {}",
                format_command(program, args),
                self.status
            )
        } else {
            format!("{} failed: {}", format_command(program, args), stderr)
        };
        Err(err(message))
    }
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
