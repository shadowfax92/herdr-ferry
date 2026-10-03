use anyhow::Result;
use clap::{Parser, Subcommand};

pub mod app;
pub mod fuzzy;
pub mod herdr;
pub mod keybindings;
pub mod kill;
pub mod kill_ops;
pub mod layout;
pub mod move_ops;
pub mod picker;
pub mod ui;

pub const PLUGIN_ID: &str = "shadowfax.ferry";

#[derive(Debug, Parser)]
#[command(name = "herdr-ferry", version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Open,
    Picker,
    InstallKeybindings,
    /// Runs a kill the picker confirmed and handed off; not meant to be run by hand.
    #[command(hide = true)]
    ExecuteKill,
}

pub fn run(command: Command) -> Result<()> {
    match command {
        Command::Open => herdr::launch_from_environment(),
        Command::Picker => picker::run_from_environment(),
        Command::InstallKeybindings => {
            keybindings::install_from_environment()?;
            Ok(())
        }
        Command::ExecuteKill => kill_ops::run_from_environment(),
    }
}
