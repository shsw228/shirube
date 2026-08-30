use std::{env, fs};

use ratatui::prelude::*;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Mode};
use crate::matcher::merged_ranges;
use crate::theme::*;

pub fn sidebar_width(app: &App) -> u16 {
    let label = app
        .groups
        .iter()
        .map(|g| g.width())
        .chain(std::iter::once("All".width()))
        .max()
        .unwrap_or(3);
    ((label + 10) as u16).clamp(14, 26)
}

pub fn sidebar_lines(app: &mut App, inner: Rect) -> Vec<Line<'static>> {
    let width = inner.width as usize;
    let height = (inner.height as usize).max(1);

    if app.group_cursor < app.group_offset {
        app.group_offset = app.group_cursor;
    }
    if app.group_cursor >= app.group_offset + height {
        app.group_offset = app.group_cursor + 1 - height;
    }

    let mut lines = Vec::new();
    for row in app.group_offset..(app.group_offset + height).min(app.group_rows()) {
        let (label, count) = if row == 0 {
            ("All".to_string(), app.total())
        } else {
            (app.groups[row - 1].clone(), app.counts[row - 1])
        };

        let selected = row == app.group_cursor;
        let active = row == app.scope;
        let empty = count == 0;

        let (name_style, count_style) = if selected {
            (selected_style(), selected_style())
        } else if empty {
            (Style::new().fg(C_MUTED), Style::new().fg(C_MUTED))
        } else if active {
            (
                Style::new().fg(C_KEY).add_modifier(Modifier::BOLD),
                Style::new().fg(C_MUTED),
            )
        } else {
            (Style::new().fg(C_TEXT), Style::new().fg(C_MUTED))
        };

        let count = count.to_string();
        let used = 2 + label.width() + count.width();
        let gap = width.saturating_sub(used).max(1);

        let line = Line::from(vec![
            Span::styled(if selected { "❯ " } else { "  " }, marker_style(selected)),
            Span::styled(label, name_style),
            Span::raw(" ".repeat(gap)),
            Span::styled(count, count_style),
        ]);
        lines.push(if selected {
            line.style(selected_style())
        } else {
            line
        });
    }
    lines
}

pub fn entry_line(app: &App, idx: usize, selected: bool) -> Line<'static> {
    let e = &app.entries[idx];

    let (owner_style, repo_style) = if selected {
        (selected_style(), selected_style())
    } else {
        (
            Style::new().fg(C_MUTED),
            Style::new().fg(C_TEXT).add_modifier(Modifier::BOLD),
        )
    };
    let hit = hit_style(selected);

    let mut spans = vec![Span::styled(
        if selected { "❯ " } else { "  " },
        marker_style(selected),
    )];

    if app.scope == 0 && app.groups.len() > 1 {
        let tag = &app.groups[e.group];
        let pad = " ".repeat(app.tag_width.saturating_sub(tag.width()));
        spans.push(Span::styled(format!("{tag}{pad}  "), owner_style));
    }

    spans.extend(highlight(
        &e.owner,
        &merged_ranges(&e.owner, &app.tokens, app.ci),
        owner_style,
        hit,
    ));
    spans.push(Span::styled(
        " ".repeat(app.owner_width.saturating_sub(e.owner.width()) + 2),
        owner_style,
    ));
    spans.extend(highlight(
        &e.repo,
        &merged_ranges(&e.repo, &app.tokens, app.ci),
        repo_style,
        hit,
    ));

    let line = Line::from(spans);
    if selected {
        line.style(selected_style())
    } else {
        line
    }
}

pub fn preview_lines(path: &str, max: usize) -> Vec<Line<'static>> {
    let Ok(rd) = fs::read_dir(path) else {
        return vec![Line::styled("(unreadable)", Style::new().fg(C_MUTED))];
    };
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for ent in rd.flatten() {
        let name = ent.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        if ent.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            dirs.push(format!("{name}/"));
        } else {
            files.push(name);
        }
    }
    dirs.sort();
    files.sort();

    let total = dirs.len() + files.len();
    let mut lines: Vec<Line> = dirs
        .into_iter()
        .map(|d| Line::styled(d, Style::new().fg(C_KEY).add_modifier(Modifier::BOLD)))
        .chain(
            files
                .into_iter()
                .map(|f| Line::styled(f, Style::new().fg(C_TEXT))),
        )
        .take(max)
        .collect();
    if total > max {
        lines.push(Line::styled(
            format!("… {} more", total - max),
            Style::new().fg(C_MUTED),
        ));
    }
    if lines.is_empty() {
        lines.push(Line::styled("(empty)", Style::new().fg(C_MUTED)));
    }
    lines
}

pub fn shorten_home(path: &str) -> String {
    match env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&home) => {
            format!("~{}", &path[home.len()..])
        }
        _ => path.to_string(),
    }
}

pub fn pending_text(app: &App) -> String {
    let mut out = String::new();
    if let Some(n) = app.count {
        out.push_str(&n.to_string());
    }
    if let Some(op) = app.operator {
        out.push(op);
    }
    out
}

pub fn status_line(app: &App) -> Line<'static> {
    let hint = Style::new().fg(C_MUTED);
    match app.mode {
        Mode::Search => Line::from(vec![
            Span::styled(" /", Style::new().fg(C_HIT).add_modifier(Modifier::BOLD)),
            Span::styled(
                app.query.clone(),
                Style::new().fg(C_TEXT).add_modifier(Modifier::BOLD),
            ),
            Span::styled("▏", Style::new().fg(C_HIT)),
            Span::styled("   enter confirm   esc cancel", hint),
        ]),
        Mode::Normal if !app.query.is_empty() => Line::from(vec![
            Span::styled(" /", hint),
            Span::styled(
                app.query.clone(),
                Style::new()
                    .fg(C_HIT)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            ),
            Span::styled(
                "   j/k move   h/l pane   / search   esc clear   enter go   q quit",
                hint,
            ),
        ]),
        Mode::Normal => Line::styled(
            " j/k move   h/l pane   tab switch   / search   enter go   q quit",
            hint,
        ),
    }
}

pub fn path_line(path: &str) -> Line<'static> {
    let shown = shorten_home(path);
    match shown.rfind('/') {
        Some(i) => Line::from(vec![
            Span::styled(format!(" {}", &shown[..=i]), Style::new().fg(C_MUTED)),
            Span::styled(
                shown[i + 1..].to_string(),
                Style::new().fg(C_TEXT).add_modifier(Modifier::BOLD),
            ),
        ]),
        None => Line::styled(format!(" {shown}"), Style::new().fg(C_TEXT)),
    }
}
