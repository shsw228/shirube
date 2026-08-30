use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Focus};
use crate::theme::*;
use crate::widgets::*;

pub struct Panes {
    pub sidebar: Option<Rect>,
    pub list: Rect,
    pub preview: Option<Rect>,
}

pub fn split_panes(area: Rect, sidebar_width: u16) -> Panes {
    let show_sidebar = area.width >= 60;
    let show_preview = area.width >= 100;

    let mut constraints: Vec<Constraint> = Vec::new();
    if show_sidebar {
        constraints.push(Constraint::Length(sidebar_width));
    }
    constraints.push(Constraint::Min(24));
    if show_preview {
        constraints.push(Constraint::Percentage(34));
    }

    let chunks = Layout::horizontal(constraints).split(area);
    let mut i = 0;
    let sidebar = show_sidebar.then(|| {
        i += 1;
        chunks[i - 1]
    });
    let list = chunks[i];
    i += 1;
    let preview = show_preview.then(|| chunks[i]);
    Panes {
        sidebar,
        list,
        preview,
    }
}

pub fn pane(title: &str, focused: bool) -> Block<'static> {
    let color = if focused { C_KEY } else { C_MUTED };
    Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::new().fg(color))
        .title(Span::styled(
            format!(" {title} "),
            Style::new().fg(color).add_modifier(Modifier::BOLD),
        ))
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let with_path = area.height >= 8;
    let footer = if with_path { 2 } else { 1 };
    let rows = Layout::vertical([Constraint::Min(3), Constraint::Length(footer)]).split(area);
    let panes = split_panes(rows[0], sidebar_width(app));

    app.sidebar_rect = None;
    if let Some(rect) = panes.sidebar {
        let block = pane("groups", app.focus == Focus::Groups);
        let inner = block.inner(rect);
        app.sidebar_rect = Some(inner);
        f.render_widget(block, rect);
        let lines = sidebar_lines(app, inner);
        f.render_widget(Paragraph::new(lines), inner);
    }

    let title = match app.scope {
        0 => format!("all · {} dirs", app.visible.len()),
        i => format!("{} · {} dirs", app.groups[i - 1], app.visible.len()),
    };
    let block = pane(&title, app.focus == Focus::List);
    let inner = block.inner(panes.list);
    f.render_widget(block, panes.list);

    let height = inner.height as usize;
    app.view_height = height.max(1);
    app.list_rect = inner;
    if height > 0 {
        if app.cursor < app.offset {
            app.offset = app.cursor;
        }
        if app.cursor >= app.offset + height {
            app.offset = app.cursor + 1 - height;
        }
        let lines: Vec<Line> = if app.visible.is_empty() {
            vec![Line::styled("  no matches", Style::new().fg(C_MUTED))]
        } else {
            (app.offset..(app.offset + height).min(app.visible.len()))
                .map(|i| entry_line(app, app.visible[i], i == app.cursor))
                .collect()
        };
        f.render_widget(Paragraph::new(lines), inner);
    }

    if let Some(rect) = panes.preview {
        let name = app.selected().map(|e| e.repo.clone()).unwrap_or_default();
        let block = pane(&name, false);
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        if let Some(p) = app.selected_path() {
            f.render_widget(
                Paragraph::new(preview_lines(p, inner.height as usize)),
                inner,
            );
        }
    }

    if with_path {
        let line = match app.selected_path() {
            Some(p) => path_line(p),
            None => Line::styled(" —", Style::new().fg(C_MUTED)),
        };
        f.render_widget(Paragraph::new(line), rows[1]);
    }
    let status_rect = Rect {
        y: rows[1].y + footer - 1,
        height: 1,
        ..rows[1]
    };
    f.render_widget(Paragraph::new(status_line(app)), status_rect);

    let pending = pending_text(app);
    if !pending.is_empty() {
        let w = pending.width() as u16 + 1;
        f.render_widget(
            Paragraph::new(Line::styled(
                format!("{pending} "),
                Style::new().fg(C_HIT).add_modifier(Modifier::BOLD),
            )),
            Rect {
                x: status_rect.x + status_rect.width.saturating_sub(w),
                width: w,
                ..status_rect
            },
        );
    }
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;
    use crate::app::Mode;
    use crate::source::{split_tail, Entry};
    use ratatui::backend::TestBackend;

    fn app_with(groups: Vec<&str>, paths: Vec<(usize, &str)>) -> App {
        App::new(
            groups.into_iter().map(str::to_string).collect(),
            paths
                .into_iter()
                .map(|(g, p)| {
                    let (owner, repo) = split_tail(p);
                    Entry {
                        path: p.to_string(),
                        group: g,
                        owner,
                        repo,
                    }
                })
                .collect(),
        )
    }

    fn render(app: &mut App, w: u16, h: u16) -> Buffer {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
        term.backend().buffer().clone()
    }

    fn text(buf: &Buffer) -> String {
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn assert_no_invisible_cells(buf: &Buffer) {
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                let cell = &buf[(x, y)];
                if cell.symbol().trim().is_empty() || cell.fg == Color::Reset {
                    continue;
                }
                assert_ne!(
                    cell.fg,
                    cell.bg,
                    "({x},{y}) の {:?} が前景と背景が同色で読めない",
                    cell.symbol()
                );
            }
        }
    }

    fn sample() -> App {
        app_with(
            vec!["ghq", "work"],
            vec![
                (0, "/g/shsw228/dotfiles"),
                (0, "/g/shsw228/shirube"),
                (0, "/g/Kyome22/GitGrass"),
                (1, "/w/work/marutope"),
            ],
        )
    }

    #[test]
    fn 端末幅でペインを出し入れする() {
        let wide = split_panes(Rect::new(0, 0, 120, 30), 16);
        assert!(wide.sidebar.is_some() && wide.preview.is_some());

        let mid = split_panes(Rect::new(0, 0, 80, 30), 16);
        assert!(mid.sidebar.is_some() && mid.preview.is_none());

        let narrow = split_panes(Rect::new(0, 0, 50, 30), 16);
        assert!(narrow.sidebar.is_none() && narrow.preview.is_none());
        assert_eq!(narrow.list.width, 50);
    }

    #[test]
    fn 読めない色の組み合わせが無い() {
        let mut a = sample();
        assert_no_invisible_cells(&render(&mut a, 110, 14));

        a.focus = Focus::Groups;
        assert_no_invisible_cells(&render(&mut a, 110, 14));

        a.focus = Focus::List;
        a.query = "shi".into();
        a.mode = Mode::Search;
        a.rebuild(true);
        assert_no_invisible_cells(&render(&mut a, 110, 14));

        assert_no_invisible_cells(&render(&mut a, 50, 10));
    }

    #[test]
    fn 選択行は背景で分かる() {
        let mut a = sample();
        let buf = render(&mut a, 110, 14);

        let cell = &buf[(15, 1)];
        assert_eq!(cell.bg, C_SEL_BG, "選択行に背景色が乗っていない");

        let below = &buf[(15, 2)];
        assert_ne!(below.bg, C_SEL_BG, "選択していない行にまで背景が乗っている");
    }

    #[test]
    fn 一致箇所に下線と色が付く() {
        let mut a = sample();
        a.query = "grass".into();
        a.rebuild(true);
        let buf = render(&mut a, 110, 14);

        let hit = (0..buf.area.width)
            .flat_map(|x| (0..buf.area.height).map(move |y| (x, y)))
            .map(|(x, y)| &buf[(x, y)])
            .find(|c| c.fg == C_HIT && c.modifier.contains(Modifier::UNDERLINED));
        assert!(hit.is_some(), "一致箇所が色と下線で示されていない");
    }

    #[test]
    fn 端末幅でペインが増減する() {
        let mut a = sample();

        let wide = text(&render(&mut a, 110, 14));
        assert!(wide.contains("groups") && wide.contains("dotfiles"));

        let mid = text(&render(&mut a, 80, 14));
        assert!(mid.contains("groups"), "80 桁ではサイドバーが出る");

        let narrow = text(&render(&mut a, 50, 14));
        assert!(!narrow.contains("groups"), "50 桁ではサイドバーを落とす");
        assert!(narrow.contains("dotfiles"), "一覧は残る");
    }

    #[test]
    fn サイドバーに全グループと件数が出る() {
        let mut a = sample();
        a.query = "marutope".into();
        a.rebuild(true);
        let out = text(&render(&mut a, 110, 14));

        assert!(out.contains("ghq"), "一致 0 でもグループは消さない");
        assert!(out.contains("work"));
        assert!(out.contains("All"));
    }

    #[test]
    fn サイドバーは縦に収まらないとスクロールする() {
        let groups: Vec<&str> = vec!["g0", "g1", "g2", "g3", "g4", "g5", "g6", "g7"];
        let paths: Vec<(usize, &str)> = vec![
            (0, "/p/a/zero"),
            (1, "/p/a/one"),
            (2, "/p/a/two"),
            (3, "/p/a/three"),
            (4, "/p/a/four"),
            (5, "/p/a/five"),
            (6, "/p/a/six"),
            (7, "/p/a/seven"),
        ];
        let mut a = app_with(groups, paths);
        a.focus = Focus::Groups;

        let top = text(&render(&mut a, 110, 8));
        assert!(top.contains("All"));
        assert!(!top.contains("g7"), "縦に収まらない分は最初は見えない");

        a.jump(true); // 末尾のグループへ
        let bottom = text(&render(&mut a, 110, 8));
        assert!(bottom.contains("g7"), "末尾のグループまで辿り着けない");
        assert!(!bottom.contains("All"), "上端はスクロールで押し出される");
    }

    #[test]
    fn 選んでいるパスが下端に出る() {
        let mut a = sample();
        let out = text(&render(&mut a, 110, 14));
        assert!(out.contains("dotfiles"));
        assert!(
            out.lines().rev().take(3).any(|l| l.contains("/shsw228/")),
            "選択中のフルパスが見えない"
        );
    }
}
