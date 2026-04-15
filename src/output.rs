use std::io::{self, Write};
use std::process::ExitCode;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RunReport {
    exit_code: u8,
    stdout: Vec<String>,
    stderr: Vec<String>,
}

impl RunReport {
    pub fn success() -> Self {
        Self::default()
    }

    pub fn stdout_line(&mut self, line: impl Into<String>) {
        self.stdout.push(line.into());
    }

    pub fn warn_line(&mut self, line: impl Into<String>) {
        self.exit_code = 1;
        self.stderr.push(format!("warning: {}", line.into()));
    }

    pub fn exit_code(&self) -> ExitCode {
        ExitCode::from(self.exit_code)
    }

    pub fn stdout(&self) -> &[String] {
        &self.stdout
    }

    pub fn stderr(&self) -> &[String] {
        &self.stderr
    }

    pub fn print(&self) -> io::Result<()> {
        let mut stdout = io::stdout().lock();
        for line in &self.stdout {
            writeln!(stdout, "{line}")?;
        }

        let mut stderr = io::stderr().lock();
        for line in &self.stderr {
            writeln!(stderr, "{line}")?;
        }

        Ok(())
    }
}
