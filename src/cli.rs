use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "grepo",
    version,
    about = "Local-first read-only reference repo store for recurring project dependencies.",
    arg_required_else_help = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Clone, Subcommand)]
pub enum Command {
    /// Create a grepo/ directory and empty lockfile in the current directory.
    Init,
    /// Register an alias pointing at a git URL and materialize it immediately.
    Add(AddArgs),
    /// Print the configured aliases and how they track upstream.
    List,
    /// Drop one or more aliases from the lockfile and delete their symlinks.
    Remove(RemoveArgs),
    /// Materialize the commits already recorded in the lockfile.
    Sync,
    /// Advance tracked aliases to their current upstream and rewrite the lockfile.
    Update(UpdateArgs),
    /// Delete cached snapshots, remote caches, and root links that no project still references.
    Gc(GcArgs),
    /// Print the bundled grepo agent skill markdown to stdout.
    Skill,
}

#[derive(Debug, Clone, Args)]
pub struct AddArgs {
    /// Short name used for the symlink under grepo/ and the lockfile key.
    #[arg(value_name = "ALIAS")]
    pub alias: String,
    /// Git URL (anything `git clone` accepts).
    #[arg(value_name = "URL")]
    pub url: String,
    /// Track a named ref (branch or tag) on the remote.
    #[arg(long = "ref", value_name = "REF", conflicts_with = "commit")]
    pub ref_name: Option<String>,
    /// Pin to an exact commit; will not advance on `update`.
    #[arg(long, value_name = "COMMIT", conflicts_with = "ref_name")]
    pub commit: Option<String>,
    /// Replace an existing alias instead of erroring out.
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Clone, Args)]
pub struct RemoveArgs {
    /// One or more aliases to remove.
    #[arg(required = true, value_name = "ALIAS")]
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, Args)]
pub struct UpdateArgs {
    /// Aliases to update; omit to update every tracking alias.
    #[arg(value_name = "ALIAS")]
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, Args)]
pub struct GcArgs {
    /// Also print each deleted path, not just the summary line.
    #[arg(long)]
    pub verbose: bool,
}

#[cfg(test)]
mod tests {
    use clap::{Parser, error::ErrorKind};

    use super::Cli;

    #[test]
    fn add_help_is_exposed_by_clap() {
        let error = Cli::try_parse_from(["grepo", "add", "-h"]).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::DisplayHelp);
        assert!(error.to_string().contains("grepo add"));
    }
}
