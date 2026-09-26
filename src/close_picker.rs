//! I/O boundary for the Close/Clear UI. The popup reads and confirms a plan;
//! after handoff it only observes a ticket. The server-owned action remains
//! responsible for completion if this popup or the invoking terminal vanishes.
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use ratatui::crossterm::event::{self, Event, KeyCode};
use ratatui::widgets::{Block, Paragraph, Wrap};

use crate::close_app::{CloseApp, CloseOutcome, Entry};
use crate::close_plan::ClosePlan;
use crate::close_ui;
use crate::close_worker::{self, JobStatus, JobStore, Ticket};
use crate::herdr::{Herdr, InvocationContext};

pub fn launch(entry: Entry) -> Result<()> {
    let herdr = Herdr::from_environment();
    let result = (|| {
        let context = InvocationContext::parse(
            &std::env::var("HERDR_PLUGIN_CONTEXT_JSON")
                .context("HERDR_PLUGIN_CONTEXT_JSON is not set")?,
        )?;
        // Resolve inherited/aliased IDs once, then store only live canonical
        // source context. Targets themselves always come from fresh topology.
        let source = herdr.pane(context.source_pane_id()?)?;
        herdr.launch_close_picker(&source.pane_id, entry)
    })();
    if let Err(error) = &result {
        let _ = herdr.notify(&format!("Could not open Close/Clear: {error:#}"));
    }
    result
}

pub fn run_from_environment() -> Result<()> {
    let herdr = Herdr::from_environment();
    let result = (|| {
        let entry = if std::env::var("HERDR_FERRY_ENTRY").as_deref() == Ok("clear-ft") {
            Entry::ClearFt
        } else {
            Entry::Close
        };
        let source =
            std::env::var("HERDR_FERRY_SOURCE_PANE_ID").context("Ferry source pane is missing")?;
        let home = std::env::var("HOME").context("HOME is missing")?;
        let mut app = CloseApp::new(herdr.topology()?, entry)?;
        if let Some(request) = app.initial_request() {
            app.set_review(ClosePlan::capture(&herdr, request, &source, &home)?);
        }
        ratatui::run(|terminal| loop {
            terminal.draw(|frame| close_ui::render(&app, frame))?;
            let event = event::read()?;
            let Event::Key(key) = event else {
                continue;
            };
            match app.handle_key(key) {
                CloseOutcome::Continue => {}
                CloseOutcome::Cancel => return Ok(()),
                CloseOutcome::Refresh => match herdr.topology() {
                    Ok(topology) => app.refresh(topology),
                    Err(e) => app.set_failure(format!("{e:#}")),
                },
                CloseOutcome::Review(request) => {
                    match ClosePlan::capture(&herdr, request, &source, &home) {
                        Ok(plan) => app.set_review(plan),
                        Err(e) => app.set_failure(format!("{e:#}. F5 refreshes targets.")),
                    }
                }
                CloseOutcome::Confirm(plan) => {
                    let submitted = plan
                        .validate(&herdr, &BTreeSet::new(), None)
                        .and_then(|_| close_worker::submit(&herdr, &plan));
                    match submitted {
                        Ok(ticket) => return observe(terminal, &ticket),
                        Err(error) => app.set_failure(format!(
                            "{error:#}. Esc goes back to select and review again."
                        )),
                    }
                }
            }
        })
    })();
    if let Err(error) = &result {
        let _ = herdr.notify(&format!("Close/Clear failed: {error:#}"));
    }
    result
}

fn observe(terminal: &mut ratatui::DefaultTerminal, ticket: &Ticket) -> Result<()> {
    let store = JobStore::from_environment()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (message, finished) = match store.status(&ticket.id)? {
            JobStatus::Finished(report) => (
                format!("{}\n{}", report.summary(), report.errors.join("\n")),
                true,
            ),
            JobStatus::Revoked => ("Handoff cancelled. Nothing was closed.".into(), true),
            JobStatus::Pending => ("Handing confirmed work to Herdr…".into(), false),
            JobStatus::Running => (
                "Confirmed closure is running. Closing this popup does not cancel it.".into(),
                false,
            ),
        };
        terminal.draw(|frame|frame.render_widget(Paragraph::new(format!("{message}\n\nReport: {}\n\nEnter / Esc closes this result. Completion is also notified.",ticket.report_path.display())).wrap(Wrap {trim:false}).block(Block::bordered().title(" Ferry · Result ")),frame.area()))?;
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('c')) {
                    return Ok(());
                }
            }
        }
        if !finished && Instant::now() >= deadline {
            // A server may acknowledge an action whose executable then fails.
            // Revoke only unclaimed work; an accepted claim is never retried.
            if store.revoke(&ticket.id)? {
                anyhow::bail!("Worker did not claim the request; nothing was closed. Check Ferry plugin logs.");
            }
            return Ok(());
        }
    }
}
