mod app;
mod cli;
mod error;
mod git;
mod manifest;
mod mutation_lock;
mod output;
mod registry;
mod store;
mod tarball;
mod util;

pub use app::main_entry;
pub use error::{GrepoError, Result};
pub use output::RunReport;

#[cfg(feature = "git-integration-tests")]
#[doc(hidden)]
pub fn run_for_test(
    cwd: std::path::PathBuf,
    cache_root: std::path::PathBuf,
    state_root: std::path::PathBuf,
    git_program: std::ffi::OsString,
    args: &[&str],
) -> Result<RunReport> {
    app::run_for_test(cwd, cache_root, state_root, git_program, args)
}
