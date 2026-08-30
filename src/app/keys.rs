use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};

use crate::app::{Action, App, Focus, Mode};
use crate::ui::view::View;

impl App {
    pub fn take_count(&mut self) -> usize {
        self.count.take().unwrap_or(1)
    }

    pub fn clear_pending(&mut self) {
        self.count = None;
        self.operator = None;
    }

    pub fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Groups => Focus::List,
            Focus::List => Focus::Groups,
        };
    }

    pub fn on_normal_key(&mut self, view: &mut View, code: KeyCode, ctrl: bool) -> Action {
        if code == KeyCode::Esc && (self.count.is_some() || self.operator.is_some()) {
            self.clear_pending();
            return Action::None;
        }

        if let Some(op) = self.operator.take() {
            let count = self.count.take();
            match (op, code) {
                ('g', KeyCode::Char('g')) => match count {
                    Some(n) => self.goto(n as isize - 1),
                    None => self.jump(false),
                },
                ('z', KeyCode::Char(c @ ('z' | 't' | 'b'))) => self.scroll_cursor_to(view, c),
                _ => {}
            }
            return Action::None;
        }

        // 数値プレフィックス。0 は単独では使わないので、開始済みのときだけ桁に足す。
        if let KeyCode::Char(c @ '0'..='9') = code {
            let d = c as usize - '0' as usize;
            if d != 0 || self.count.is_some() {
                self.count = Some(self.count.unwrap_or(0) * 10 + d);
                return Action::None;
            }
        }

        match code {
            KeyCode::Char('g') | KeyCode::Char('z') => {
                self.operator = Some(if matches!(code, KeyCode::Char('g')) {
                    'g'
                } else {
                    'z'
                });
                return Action::None;
            }
            _ => {}
        }

        let counted = self.count.is_some();
        let count = self.take_count();
        let page = view.height.max(1);

        match code {
            KeyCode::Char('q') => return Action::Quit,
            KeyCode::Esc => {
                if self.query.is_empty() {
                    return Action::Quit;
                }
                self.query.clear();
                self.rebuild(true);
                view.reset_scroll();
            }
            KeyCode::Enter => match self.focus {
                Focus::Groups => self.focus = Focus::List,
                Focus::List => return Action::Choose,
            },
            KeyCode::Char('/') => self.mode = Mode::Search,

            KeyCode::Char('j') | KeyCode::Down => self.step(count as isize),
            KeyCode::Char('k') | KeyCode::Up => self.step(-(count as isize)),
            KeyCode::Char('n') if ctrl => self.step(count as isize),
            KeyCode::Char('p') if ctrl => self.step(-(count as isize)),

            KeyCode::Char('G') if counted => self.goto(count as isize - 1),
            KeyCode::Char('G') | KeyCode::End => self.jump(true),
            KeyCode::Home => self.jump(false),

            KeyCode::Char('d') if ctrl => self.step((page / 2).max(1) as isize),
            KeyCode::Char('u') if ctrl => self.step(-((page / 2).max(1) as isize)),
            KeyCode::Char('f') if ctrl => self.step(page as isize),
            KeyCode::Char('b') if ctrl => self.step(-(page as isize)),
            KeyCode::PageDown => self.step(page as isize),
            KeyCode::PageUp => self.step(-(page as isize)),

            KeyCode::Char('e') if ctrl => self.scroll_view(view, count as isize),
            KeyCode::Char('y') if ctrl => self.scroll_view(view, -(count as isize)),

            KeyCode::Char(c @ ('H' | 'M' | 'L')) => self.jump_screen(view, c),
            KeyCode::Char('}') => self.jump_group_edge(true),
            KeyCode::Char('{') => self.jump_group_edge(false),

            KeyCode::Char('h') | KeyCode::Left => self.focus = Focus::Groups,
            KeyCode::Char('l') | KeyCode::Right => self.focus = Focus::List,
            KeyCode::Tab | KeyCode::BackTab => self.toggle_focus(),
            _ => {}
        }
        Action::None
    }

    pub fn on_mouse(&mut self, view: &View, kind: MouseEventKind, col: u16, row: u16) -> Action {
        let over_sidebar = view.sidebar.is_some_and(|r| view.contains(r, col, row));
        let over_list = view.contains(view.list, col, row);

        match kind {
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let delta = if kind == MouseEventKind::ScrollDown {
                    3
                } else {
                    -3
                };
                if over_sidebar {
                    let keep = self.focus;
                    self.focus = Focus::Groups;
                    self.step(delta);
                    self.focus = keep;
                } else {
                    self.step_list(delta);
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if over_sidebar {
                    let rect = view.sidebar.unwrap();
                    let i = view.group_offset + (row - rect.y) as usize;
                    if i < self.group_rows() {
                        self.focus = Focus::Groups;
                        self.goto(i as isize);
                    }
                } else if over_list {
                    let i = view.offset + (row - view.list.y) as usize;
                    if i < self.visible.len() {
                        let again = self.focus == Focus::List && self.cursor == i;
                        self.focus = Focus::List;
                        self.cursor = i;
                        if again {
                            return Action::Choose;
                        }
                    }
                }
            }
            _ => {}
        }
        Action::None
    }

    pub fn on_search_key(&mut self, view: &mut View, code: KeyCode, ctrl: bool) -> Action {
        match code {
            KeyCode::Enter => self.mode = Mode::Normal,
            KeyCode::Esc => {
                self.query.clear();
                self.mode = Mode::Normal;
                self.rebuild(true);
                view.reset_scroll();
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.rebuild(true);
                view.reset_scroll();
            }
            KeyCode::Char('u') if ctrl => {
                self.query.clear();
                self.rebuild(true);
                view.reset_scroll();
            }
            KeyCode::Char('w') if ctrl => {
                while self.query.ends_with(' ') {
                    self.query.pop();
                }
                while !self.query.is_empty() && !self.query.ends_with(' ') {
                    self.query.pop();
                }
                self.rebuild(true);
                view.reset_scroll();
            }
            KeyCode::Up => self.step_list(-1),
            KeyCode::Down => self.step_list(1),
            KeyCode::Char('p') if ctrl => self.step_list(-1),
            KeyCode::Char('n') if ctrl => self.step_list(1),
            KeyCode::Char(c) => {
                self.query.push(c);
                self.rebuild(true);
                view.reset_scroll();
            }
            _ => {}
        }
        Action::None
    }
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;
    use crate::model::{split_tail, Entry};
    use crate::ui::view::View;

    fn entry(group: usize, path: &str) -> Entry {
        let (owner, repo) = split_tail(path);
        Entry {
            path: path.to_string(),
            group,
            owner,
            repo,
        }
    }

    fn key(a: &mut App, v: &mut View, k: char) -> Action {
        a.on_normal_key(v, KeyCode::Char(k), false)
    }

    fn keys(a: &mut App, v: &mut View, s: &str) {
        for c in s.chars() {
            key(a, v, c);
        }
    }

    fn big() -> (App, View) {
        let entries = (0..30)
            .map(|i| entry(if i < 20 { 0 } else { 1 }, &format!("/g/own{i}/repo{i}")))
            .collect();
        let a = App::new(vec!["ghq".into(), "work".into()], entries);
        let view = View {
            height: 10,
            ..View::default()
        };
        (a, view)
    }

    #[test]
    fn ggとGで端に飛ぶ() {
        let (mut a, mut v) = big();
        keys(&mut a, &mut v, "G");
        assert_eq!(a.cursor, 29);
        keys(&mut a, &mut v, "gg");
        assert_eq!(a.cursor, 0);
    }

    #[test]
    fn gは一打鍵では動かない() {
        let (mut a, mut v) = big();
        keys(&mut a, &mut v, "G");
        keys(&mut a, &mut v, "g");
        assert_eq!(a.cursor, 29, "g だけでは確定しない");
        assert_eq!(a.operator, Some('g'));
        keys(&mut a, &mut v, "g");
        assert_eq!(a.cursor, 0);
        assert_eq!(a.operator, None);
    }

    #[test]
    fn 数値プレフィックスが効く() {
        let (mut a, mut v) = big();
        keys(&mut a, &mut v, "5j");
        assert_eq!(a.cursor, 5);
        keys(&mut a, &mut v, "12G");
        assert_eq!(a.cursor, 11, "12G は 12 行目");
        keys(&mut a, &mut v, "3gg");
        assert_eq!(a.cursor, 2, "3gg も行指定");
        keys(&mut a, &mut v, "2k");
        assert_eq!(a.cursor, 0, "端で止まる");
    }

    #[test]
    fn Escで打ちかけの入力を取り消す() {
        let (mut a, mut v) = big();
        keys(&mut a, &mut v, "5");
        assert_eq!(a.count, Some(5));
        assert_eq!(a.on_normal_key(&mut v, KeyCode::Esc, false), Action::None);
        assert_eq!(a.count, None, "終了ではなく取り消し");
        keys(&mut a, &mut v, "j");
        assert_eq!(a.cursor, 1, "取り消した 5 は残らない");
    }

    #[test]
    fn HMLは画面内の位置へ飛ぶ() {
        let (mut a, mut v) = big();
        v.offset = 10;
        a.cursor = 12;
        keys(&mut a, &mut v, "H");
        assert_eq!(a.cursor, 10);
        keys(&mut a, &mut v, "L");
        assert_eq!(a.cursor, 19);
        keys(&mut a, &mut v, "M");
        assert_eq!(a.cursor, 14);
    }

    #[test]
    fn zzztzbで表示位置を変える() {
        let (mut a, mut v) = big();
        a.cursor = 20;
        keys(&mut a, &mut v, "zt");
        assert_eq!(v.offset, 20);
        keys(&mut a, &mut v, "zb");
        assert_eq!(v.offset, 11);
        keys(&mut a, &mut v, "zz");
        assert_eq!(v.offset, 15);
    }

    #[test]
    fn 波括弧でグループの切れ目へ飛ぶ() {
        let (mut a, mut v) = big();
        keys(&mut a, &mut v, "}");
        assert_eq!(a.cursor, 20, "次のグループの先頭");
        keys(&mut a, &mut v, "}");
        assert_eq!(a.cursor, 29, "先が無ければ末尾");
        keys(&mut a, &mut v, "{");
        assert_eq!(a.cursor, 20, "まず今のグループの先頭");
        keys(&mut a, &mut v, "{");
        assert_eq!(a.cursor, 0, "次に前のグループの先頭");
    }

    #[test]
    fn ページ移動は一覧の高さに従う() {
        let (mut a, mut v) = big();
        a.on_normal_key(&mut v, KeyCode::Char('d'), true);
        assert_eq!(a.cursor, 5, "Ctrl-d は半ページ");
        a.on_normal_key(&mut v, KeyCode::Char('f'), true);
        assert_eq!(a.cursor, 15, "Ctrl-f は 1 ページ");
        a.on_normal_key(&mut v, KeyCode::Char('b'), true);
        assert_eq!(a.cursor, 5);
    }

    #[test]
    fn qとEnterの結果が呼び出し側に返る() {
        let (mut a, mut v) = big();
        assert_eq!(
            a.on_normal_key(&mut v, KeyCode::Enter, false),
            Action::Choose
        );
        assert_eq!(key(&mut a, &mut v, 'q'), Action::Quit);

        a.focus = Focus::Groups;
        assert_eq!(a.on_normal_key(&mut v, KeyCode::Enter, false), Action::None);
        assert_eq!(a.focus, Focus::List, "サイドバーからは一覧へ移るだけ");
    }
}
