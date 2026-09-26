use anyhow::Result;
use clap::{Parser, Subcommand};

pub mod app;
pub mod close_app;
pub mod close_ops;
pub mod close_picker;
pub mod close_plan;
pub mod close_ui;
pub mod close_worker;
pub mod fuzzy;
pub mod herdr;
pub mod keybindings;
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
    OpenClose,
    ClearFt,
    #[command(hide = true)]
    ClosePicker,
    Picker,
    #[command(hide = true)]
    CloseWorker,
    InstallKeybindings,
}

pub fn run(command: Command) -> Result<()> {
    match command {
        Command::Open => herdr::launch_from_environment(),
        Command::OpenClose => close_picker::launch(close_app::Entry::Close),
        Command::ClearFt => close_picker::launch(close_app::Entry::ClearFt),
        Command::ClosePicker => close_picker::run_from_environment(),
        Command::Picker => picker::run_from_environment(),
        Command::CloseWorker => close_worker::run_from_environment(),
        Command::InstallKeybindings => {
            keybindings::install_from_environment()?;
            Ok(())
        }
    }
}
