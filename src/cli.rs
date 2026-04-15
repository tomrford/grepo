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
    Init,
    Add(AddArgs),
    Remove(RemoveArgs),
    Sync,
    Update(UpdateArgs),
    Gc,
}

#[derive(Debug, Clone, Args)]
pub struct AddArgs {
    #[arg(value_name = "ALIAS")]
    pub alias: String,
    #[arg(value_name = "URL")]
    pub url: String,
    #[arg(long = "ref", value_name = "REF", conflicts_with = "commit")]
    pub ref_name: Option<String>,
    #[arg(long, value_name = "COMMIT", conflicts_with = "ref_name")]
    pub commit: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct RemoveArgs {
    #[arg(required = true, value_name = "ALIAS")]
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, Args)]
pub struct UpdateArgs {
    #[arg(value_name = "ALIAS")]
    pub aliases: Vec<String>,
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
