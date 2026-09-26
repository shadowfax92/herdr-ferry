//! Native Close/Clear presentation. Reviews wrap and scroll independently of
//! confirmation input, keeping exact identities and programs accessible even
//! when a selected workspace contains more panes than fit in the popup.
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use crate::close_app::CloseApp;

pub fn render(app: &CloseApp, frame: &mut Frame) {
    let border = Block::bordered()
        .title(" Ferry · Close / Clear ")
        .border_style(Style::default().fg(Color::Cyan));
    let inner = border.inner(frame.area());
    frame.render_widget(border, frame.area());
    let [heading, input, body, status, footer] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new(app.heading())
            .style(Style::default().add_modifier(Modifier::BOLD))
            .wrap(Wrap { trim: false }),
        heading,
    );
    if app.review().is_some() {
        frame.render_widget(
            Paragraph::new(format!(
                "Type {} then Enter: {}",
                app.confirm_word(),
                app.confirmation()
            ))
            .style(Style::default().fg(Color::Yellow)),
            input,
        );
        let text = app.review_text();
        // Pre-wrap into display-width chunks so scrolling and bounds use the
        // exact same rows without depending on unstable widget measurement API.
        let lines = wrap_lines(&text, usize::from(body.width));
        let max_scroll = lines.len().saturating_sub(usize::from(body.height));
        let scroll = usize::from(app.scroll())
            .min(max_scroll)
            .min(u16::MAX as usize) as u16;
        frame.render_widget(Paragraph::new(lines.join("\n")).scroll((scroll, 0)), body);
        frame.render_widget(
            Paragraph::new("↑↓ / PgUp/PgDn scroll   esc back   ctrl+c cancel"),
            footer,
        );
    } else {
        if !app.is_kind() {
            frame.render_widget(Paragraph::new(format!("Search: {}", app.query())), input);
        }
        let items = app
            .rows()
            .into_iter()
            .map(|row| {
                ListItem::new(vec![
                    Line::from(vec![
                        Span::styled(
                            if row.checked { "✓ " } else { "  " },
                            Style::default().fg(Color::Cyan),
                        ),
                        Span::raw(row.title),
                    ]),
                    Line::styled(
                        format!("  {}", row.detail),
                        Style::default().fg(Color::DarkGray),
                    ),
                ])
            })
            .collect::<Vec<_>>();
        let list = List::new(items)
            .highlight_style(Style::default().bg(Color::Blue))
            .highlight_symbol("▌ ")
            .scroll_padding(1);
        frame.render_stateful_widget(
            list,
            body,
            &mut ListState::default().with_selected(Some(app.selected())),
        );
        let hint = if app.is_kind() {
            "p/t/w/c choose   ↑↓ navigate   enter choose   esc close"
        } else {
            "space/tab select   ctrl+a all   enter review   F5 refresh   esc back"
        };
        frame.render_widget(Paragraph::new(hint), footer);
    }
    if let Some(error) = app.failure() {
        frame.render_widget(
            Paragraph::new(error)
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(Color::LightRed)),
            status,
        );
    }
}

fn wrap_lines(text: &str, width: usize) -> Vec<String> {
    use unicode_width::UnicodeWidthChar;
    let width = width.max(1);
    let mut rows = Vec::new();
    for line in text.lines() {
        let mut row = String::new();
        let mut used = 0;
        for c in line.chars() {
            let size = c.width().unwrap_or(0);
            if used + size > width && !row.is_empty() {
                rows.push(std::mem::take(&mut row));
                used = 0;
            }
            row.push(c);
            used += size;
        }
        rows.push(row);
    }
    rows
}
