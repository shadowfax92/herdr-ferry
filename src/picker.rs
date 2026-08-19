use anyhow::{Context, Result};
use ratatui::crossterm::event::{self, Event};

use crate::app::{App, InputOutcome};
use crate::herdr::Herdr;
use crate::move_ops::Mover;
use crate::ui;

pub fn run_from_environment() -> Result<()> {
    let herdr = Herdr::from_environment();
    let result = (|| {
        let source_pane_id = std::env::var("HERDR_FERRY_SOURCE_PANE_ID")
            .context("HERDR_FERRY_SOURCE_PANE_ID is not set")?;
        let topology = herdr.topology()?;
        let mut app = App::new(topology, source_pane_id)?;
        let mover = Mover::new(herdr.clone());
        ratatui::run(|terminal| run_picker(terminal, &mut app, &mover, &herdr))
            .context("Ferry picker failed")
    })();
    if let Err(error) = &result {
        let _ = herdr.notify(&format!("Ferry failed: {error:#}"));
    }
    result
}

fn run_picker(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    mover: &Mover,
    herdr: &Herdr,
) -> Result<()> {
    loop {
        terminal.draw(|frame| ui::render(app, frame))?;
        let event = event::read()?;
        if matches!(event, Event::Resize(_, _)) {
            terminal.autoresize()?;
            continue;
        }
        match app.handle_event(event) {
            InputOutcome::Continue => {}
            InputOutcome::Cancel => return Ok(()),
            InputOutcome::MovePane(request) => {
                let noun = if request.sources.len() == 1 {
                    "pane"
                } else {
                    "panes"
                };
                app.set_working(format!("Moving {} {noun}…", request.sources.len()));
                terminal.draw(|frame| ui::render(app, frame))?;
                match mover.move_pane(&request) {
                    Ok(summary) => {
                        let _ = herdr.notify(&summary.message);
                        return Ok(());
                    }
                    Err(error) => app.set_failure(format!("{error:#}")),
                }
            }
            InputOutcome::MoveTab(request) => {
                let subject = if request.tab_ids.len() == 1 {
                    "tab with its"
                } else {
                    "tabs with their"
                };
                app.set_working(format!(
                    "Moving {} {subject} live panes…",
                    request.tab_ids.len()
                ));
                terminal.draw(|frame| ui::render(app, frame))?;
                match mover.move_tab(&request) {
                    Ok(summary) => {
                        let _ = herdr.notify(&summary.message);
                        return Ok(());
                    }
                    Err(error) => app.set_failure(format!("{error:#}")),
                }
            }
            InputOutcome::MergeWorkspace(request) => {
                app.set_working("Appending every source tab to the destination…");
                terminal.draw(|frame| ui::render(app, frame))?;
                match mover.merge_workspace(&request) {
                    Ok(summary) => {
                        let _ = herdr.notify(&summary.message);
                        return Ok(());
                    }
                    Err(error) => app.set_failure(format!("{error:#}")),
                }
            }
        }
    }
}
