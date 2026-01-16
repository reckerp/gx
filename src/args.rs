use crate::commands;
use clap::{Parser, Subcommand};
use miette::Result;

#[derive(Parser)]
#[command(name = "gx", about = "GX - Smart Git CLI", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Checkout/Switch a branch|commit|tag
    #[command(alias = "co", aliases = ["switch"])]
    Checkout { query: Option<String> },

    /// Pass-through to git for unrecognized commands
    #[command(external_subcommand)]
    External(Vec<String>),
}

impl Commands {
    pub fn run(self) -> Result<()> {
        match self {
            Self::Checkout { query } => commands::checkout::run(query),
            Self::External(args) => commands::external::run(args),
        }
    }
}
