use clap::{ArgGroup, Args, Parser, Subcommand};

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
    /// Register an alias and materialize it immediately.
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
#[command(group(
    ArgGroup::new("source")
        .required(true)
        .args(["url", "npm", "cargo_pkg"])
))]
pub struct AddArgs {
    /// Short name used for the symlink under grepo/ and the lockfile key.
    #[arg(value_name = "ALIAS")]
    pub alias: String,
    /// Git URL (anything `git clone` accepts).
    #[arg(long, value_name = "URL")]
    pub url: Option<String>,
    /// npm package spec, exactly as you would pass to `npm install`
    /// (e.g. "react", "react@18.2.0", "@types/node@20.10.0").
    #[arg(long, value_name = "SPEC")]
    pub npm: Option<String>,
    /// Cargo crate spec, exactly as you would pass to `cargo add`
    /// (e.g. "serde", "serde@1.0.197").
    #[arg(long = "cargo", value_name = "SPEC")]
    pub cargo_pkg: Option<String>,
    /// Track a named ref (branch or tag) on the remote. Only valid with --url.
    #[arg(long = "ref", value_name = "REF", conflicts_with_all = ["commit", "npm", "cargo_pkg"])]
    pub ref_name: Option<String>,
    /// Pin to an exact commit; will not advance on `update`. Only valid with --url.
    #[arg(long, value_name = "COMMIT", conflicts_with_all = ["ref_name", "npm", "cargo_pkg"])]
    pub commit: Option<String>,
    /// Pick a subdirectory of the resolved source as the snapshot root.
    /// Invalid with --cargo.
    #[arg(long, value_name = "PATH", conflicts_with = "cargo_pkg")]
    pub subdir: Option<String>,
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

    use super::{Cli, Command};

    #[test]
    fn add_help_is_exposed_by_clap() {
        let error = Cli::try_parse_from(["grepo", "add", "-h"]).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::DisplayHelp);
        assert!(error.to_string().contains("grepo add"));
    }

    #[test]
    fn add_requires_exactly_one_source_flag() {
        let error = Cli::try_parse_from(["grepo", "add", "react", "--npm", "react", "--url", "u"])
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::ArgumentConflict);

        let error = Cli::try_parse_from(["grepo", "add", "react"]).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn add_parses_url_with_ref_and_subdir() {
        let cli = Cli::try_parse_from([
            "grepo",
            "add",
            "mint",
            "--url",
            "git@example.com:x.git",
            "--ref",
            "main",
            "--subdir",
            "pkg/core",
        ])
        .unwrap();
        let Command::Add(args) = cli.command else {
            panic!();
        };
        assert_eq!(args.alias, "mint");
        assert_eq!(args.url.as_deref(), Some("git@example.com:x.git"));
        assert_eq!(args.ref_name.as_deref(), Some("main"));
        assert_eq!(args.subdir.as_deref(), Some("pkg/core"));
    }

    #[test]
    fn add_parses_npm_spec() {
        let cli = Cli::try_parse_from(["grepo", "add", "react", "--npm", "react@18.2.0"]).unwrap();
        let Command::Add(args) = cli.command else {
            panic!();
        };
        assert_eq!(args.npm.as_deref(), Some("react@18.2.0"));
    }

    #[test]
    fn add_parses_cargo_spec() {
        let cli =
            Cli::try_parse_from(["grepo", "add", "serde", "--cargo", "serde@1.0.197"]).unwrap();
        let Command::Add(args) = cli.command else {
            panic!();
        };
        assert_eq!(args.cargo_pkg.as_deref(), Some("serde@1.0.197"));
    }

    #[test]
    fn add_rejects_subdir_with_cargo() {
        let error = Cli::try_parse_from([
            "grepo", "add", "serde", "--cargo", "serde@1", "--subdir", "x",
        ])
        .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::ArgumentConflict);
    }

    #[test]
    fn add_rejects_ref_with_npm() {
        let error =
            Cli::try_parse_from(["grepo", "add", "react", "--npm", "react", "--ref", "main"])
                .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::ArgumentConflict);
    }
}
