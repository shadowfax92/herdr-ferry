use ratatui::layout::{Constraint, Layout, Position};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, RowTone};

pub fn render(app: &App, frame: &mut Frame) {
    let area = frame.area();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Line::from(vec![
            Span::styled(
                " Ferry ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{}/3 ", app.step()),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let prompt_height = u16::from(app.is_searchable());
    let [trail_area, heading_area, prompt_area, list_area, status_area, footer_area] =
        Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Length(prompt_height),
            Constraint::Fill(1),
            Constraint::Length(2),
            Constraint::Length(1),
        ])
        .areas(inner);

    frame.render_widget(
        Paragraph::new(app.trail()).style(Style::default().fg(Color::DarkGray)),
        trail_area,
    );
    frame.render_widget(
        Paragraph::new(app.heading()).style(
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        heading_area,
    );

    if app.is_searchable() {
        let query = if app.query().is_empty() {
            Span::styled(app.prompt(), Style::default().fg(Color::DarkGray))
        } else {
            Span::styled(app.query(), Style::default().fg(Color::White))
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("› ", Style::default().fg(Color::Cyan)),
                query,
            ])),
            prompt_area,
        );
        let cursor_offset = 2 + UnicodeWidthStr::width(app.query());
        let cursor_x = prompt_area
            .x
            .saturating_add(u16::try_from(cursor_offset).unwrap_or(u16::MAX))
            .min(prompt_area.right().saturating_sub(1));
        frame.set_cursor_position(Position::new(cursor_x, prompt_area.y));
    }

    let rows = app.rows();
    if rows.is_empty() {
        frame.render_widget(
            Paragraph::new("  No matches").style(Style::default().fg(Color::DarkGray)),
            list_area,
        );
    } else {
        let items = rows
            .into_iter()
            .map(|row| {
                let (marker, marker_style) = if row.checked {
                    (
                        "✓ ",
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    )
                } else {
                    match row.tone {
                        RowTone::Normal => ("  ", Style::default()),
                        RowTone::Current => ("● ", Style::default().fg(Color::Green)),
                        RowTone::Create => ("＋ ", Style::default().fg(Color::Cyan)),
                    }
                };
                ListItem::new(Line::from(vec![
                    Span::styled(marker, marker_style),
                    Span::styled(row.title, Style::default().fg(Color::White)),
                    Span::styled("  ", Style::default()),
                    Span::styled(row.detail, Style::default().fg(Color::DarkGray)),
                ]))
            })
            .collect::<Vec<_>>();
        let list = List::new(items)
            .highlight_style(
                Style::default()
                    .bg(Color::Blue)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▌ ")
            .scroll_padding(2);
        let mut state = ListState::default().with_selected(app.selected());
        frame.render_stateful_widget(list, list_area, &mut state);
    }

    let status = if let Some(error) = app.failure() {
        Paragraph::new(format!("Move failed: {error}"))
            .style(Style::default().fg(Color::LightRed))
            .wrap(Wrap { trim: true })
    } else if let Some(working) = app.working() {
        Paragraph::new(working).style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Paragraph::new("")
    };
    frame.render_widget(status, status_area);
    frame.render_widget(
        Paragraph::new(app.footer()).style(Style::default().fg(Color::DarkGray)),
        footer_area,
    );
}

#[cfg(test)]
mod tests {
    use ratatui::{backend::TestBackend, Terminal};

    use crate::herdr::{PaneInfo, TabInfo, Topology, WorkspaceInfo};

    use super::*;

    fn app() -> App {
        App::new(
            Topology {
                workspaces: vec![WorkspaceInfo {
                    workspace_id: "w1".into(),
                    label: "source".into(),
                    number: 1,
                    tab_count: 1,
                    pane_count: 1,
                    focused: true,
                }],
                tabs: vec![TabInfo {
                    tab_id: "w1:t1".into(),
                    workspace_id: "w1".into(),
                    label: "main".into(),
                    number: 1,
                    pane_count: 1,
                    focused: true,
                }],
                panes: vec![PaneInfo {
                    pane_id: "w1:p1".into(),
                    tab_id: "w1:t1".into(),
                    workspace_id: "w1".into(),
                    label: Some("agent".into()),
                    terminal_title_stripped: None,
                    cwd: None,
                    agent: Some("codex".into()),
                    agent_status: "working".into(),
                    focused: true,
                }],
            },
            "w1:p1",
        )
        .unwrap()
    }

    fn render_text(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(76, 24)).unwrap();
        terminal.draw(|frame| render(app, frame)).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn renders_the_three_step_entry_screen() {
        let rendered = render_text(&app());

        assert!(rendered.contains("Ferry"));
        assert!(rendered.contains("What should cross?"));
        assert!(rendered.contains("Move a pane"));
        assert!(rendered.contains("Move a whole tab"));
        assert!(rendered.contains("p/t shortcut"));
    }

    #[test]
    fn renders_failures_without_leaving_the_popup() {
        let mut app = app();
        app.set_failure("source changed");

        assert!(render_text(&app).contains("Move failed: source changed"));
    }
}
