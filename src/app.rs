use ratatui::prelude::Rect;
use unicode_width::UnicodeWidthStr;

use crate::matcher::find_all;
use crate::source::Entry;

#[derive(PartialEq)]
pub enum Mode {
    Normal,
    Search,
}

#[derive(PartialEq, Debug)]
pub enum Action {
    None,
    Quit,
    Choose,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Focus {
    Groups,
    List,
}

pub struct App {
    pub groups: Vec<String>,
    pub entries: Vec<Entry>,
    pub query: String,
    pub tokens: Vec<String>,
    pub ci: bool,
    pub mode: Mode,
    pub focus: Focus,
    pub scope: usize,
    pub counts: Vec<usize>,
    pub visible: Vec<usize>,
    pub cursor: usize,
    pub offset: usize,
    pub group_cursor: usize,
    pub group_offset: usize,
    pub owner_width: usize,
    pub tag_width: usize,
    pub count: Option<usize>,
    pub operator: Option<char>,
    pub view_height: usize,
    pub list_rect: Rect,
    pub sidebar_rect: Option<Rect>,
}

impl App {
    pub fn new(groups: Vec<String>, entries: Vec<Entry>) -> Self {
        let counts = vec![0; groups.len()];
        let mut app = App {
            groups,
            entries,
            query: String::new(),
            tokens: Vec::new(),
            ci: true,
            mode: Mode::Normal,
            focus: Focus::List,
            scope: 0,
            counts,
            visible: Vec::new(),
            cursor: 0,
            offset: 0,
            group_cursor: 0,
            group_offset: 0,
            owner_width: 0,
            tag_width: 0,
            count: None,
            operator: None,
            view_height: 1,
            list_rect: Rect::default(),
            sidebar_rect: None,
        };
        app.rebuild(true);
        app
    }

    pub fn matches(&self, e: &Entry) -> bool {
        self.tokens.iter().all(|t| {
            !find_all(&e.repo, t, self.ci).is_empty()
                || !find_all(&e.owner, t, self.ci).is_empty()
                || !find_all(&self.groups[e.group], t, self.ci).is_empty()
        })
    }

    pub fn rebuild(&mut self, reset_cursor: bool) {
        self.tokens = self.query.split_whitespace().map(str::to_string).collect();
        self.ci = !self.query.chars().any(char::is_uppercase);

        self.counts.fill(0);
        self.visible.clear();
        for (i, e) in self.entries.iter().enumerate() {
            if !self.matches(e) {
                continue;
            }
            self.counts[e.group] += 1;
            if self.scope == 0 || self.scope == e.group + 1 {
                self.visible.push(i);
            }
        }

        self.owner_width = self
            .visible
            .iter()
            .map(|&i| self.entries[i].owner.width())
            .max()
            .unwrap_or(0);
        self.tag_width = self
            .visible
            .iter()
            .map(|&i| self.groups[self.entries[i].group].width())
            .max()
            .unwrap_or(0);

        if reset_cursor {
            self.cursor = 0;
            self.offset = 0;
        }
        self.cursor = self.cursor.min(self.visible.len().saturating_sub(1));
    }

    pub fn total(&self) -> usize {
        self.counts.iter().sum()
    }

    pub fn group_rows(&self) -> usize {
        self.groups.len() + 1
    }

    pub fn set_scope(&mut self, scope: usize) {
        if self.scope == scope {
            return;
        }
        self.scope = scope;
        self.rebuild(true);
    }

    pub fn len_focused(&self) -> usize {
        match self.focus {
            Focus::List => self.visible.len(),
            Focus::Groups => self.group_rows(),
        }
    }

    pub fn cursor_focused(&self) -> usize {
        match self.focus {
            Focus::List => self.cursor,
            Focus::Groups => self.group_cursor,
        }
    }

    pub fn goto(&mut self, index: isize) {
        let len = self.len_focused();
        if len == 0 {
            return;
        }
        let i = index.clamp(0, len as isize - 1) as usize;
        match self.focus {
            Focus::List => self.cursor = i,
            Focus::Groups => {
                self.group_cursor = i;
                self.set_scope(i);
            }
        }
    }

    pub fn step_list(&mut self, delta: isize) {
        if self.visible.is_empty() {
            return;
        }
        let last = self.visible.len() as isize - 1;
        self.cursor = (self.cursor as isize + delta).clamp(0, last) as usize;
    }

    pub fn step(&mut self, delta: isize) {
        self.goto(self.cursor_focused() as isize + delta);
    }

    pub fn jump(&mut self, to_end: bool) {
        let last = self.len_focused() as isize - 1;
        self.goto(if to_end { last } else { 0 });
    }

    pub fn jump_screen(&mut self, where_to: char) {
        if self.focus != Focus::List || self.visible.is_empty() {
            return;
        }
        let last_visible =
            (self.offset + self.view_height.saturating_sub(1)).min(self.visible.len() - 1);
        self.cursor = match where_to {
            'H' => self.offset,
            'L' => last_visible,
            _ => (self.offset + last_visible) / 2,
        };
    }

    pub fn scroll_cursor_to(&mut self, where_to: char) {
        if self.focus != Focus::List {
            return;
        }
        let h = self.view_height.max(1);
        self.offset = match where_to {
            't' => self.cursor,
            'b' => self.cursor.saturating_sub(h - 1),
            _ => self.cursor.saturating_sub(h / 2),
        };
    }

    pub fn scroll_view(&mut self, delta: isize) {
        if self.focus != Focus::List || self.visible.is_empty() {
            return;
        }
        let last = self.visible.len().saturating_sub(1);
        self.offset = (self.offset as isize + delta).clamp(0, last as isize) as usize;
        let bottom = self.offset + self.view_height.saturating_sub(1);
        self.cursor = self.cursor.clamp(self.offset, bottom.min(last));
    }

    pub fn jump_group_edge(&mut self, forward: bool) {
        if self.focus != Focus::List || self.visible.is_empty() {
            return;
        }
        let group_of = |i: usize| self.entries[self.visible[i]].group;
        let here = group_of(self.cursor);
        if forward {
            for i in self.cursor + 1..self.visible.len() {
                if group_of(i) != here {
                    self.cursor = i;
                    return;
                }
            }
            self.cursor = self.visible.len() - 1;
        } else {
            // まず自分のグループの先頭へ、既にそこなら一つ前のグループの先頭へ。
            let start = (0..=self.cursor)
                .rev()
                .take_while(|&i| group_of(i) == here)
                .last()
                .unwrap_or(self.cursor);
            if start < self.cursor {
                self.cursor = start;
            } else if start > 0 {
                let prev = group_of(start - 1);
                self.cursor = (0..start)
                    .rev()
                    .take_while(|&i| group_of(i) == prev)
                    .last()
                    .unwrap_or(0);
            } else {
                self.cursor = 0;
            }
        }
    }

    pub fn selected(&self) -> Option<&Entry> {
        self.visible.get(self.cursor).map(|&i| &self.entries[i])
    }

    pub fn selected_path(&self) -> Option<&str> {
        self.selected().map(|e| e.path.as_str())
    }
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;
    use crate::source::split_tail;

    fn entry(group: usize, path: &str) -> Entry {
        let (owner, repo) = split_tail(path);
        Entry {
            path: path.to_string(),
            group,
            owner,
            repo,
        }
    }

    fn app() -> App {
        App::new(
            vec!["ghq".into(), "work".into()],
            vec![
                entry(0, "/g/github.com/shsw228/PhotoScrubberKit"),
                entry(0, "/g/github.com/shsw228/portal-available-checker"),
                entry(0, "/g/github.com/Kyome22/LUCA-Skills"),
                entry(1, "/w/work/marutope"),
                entry(1, "/w/work/marutope/受注A"),
            ],
        )
    }

    fn shown(a: &App) -> Vec<&str> {
        a.visible
            .iter()
            .map(|&i| a.entries[i].repo.as_str())
            .collect()
    }

    #[test]
    fn 散らばった文字ではマッチしない() {
        let mut a = app();
        a.query = "work".into();
        a.rebuild(true);
        assert_eq!(shown(&a), vec!["marutope", "受注A"]);
    }

    #[test]
    fn 空白区切りは全語を含むものだけに絞る() {
        let mut a = app();
        a.query = "maru 受注".into();
        a.rebuild(true);
        assert_eq!(shown(&a), vec!["受注A"]);
    }

    #[test]
    fn 絞り込んでも全グループが件数付きで残る() {
        let mut a = app();
        a.query = "maru".into();
        a.rebuild(true);
        assert_eq!(a.counts.len(), a.groups.len());
        assert_eq!(a.counts, vec![0, 2]);
        assert_eq!(a.group_rows(), 3, "All とグループ 2 つ");
        assert_eq!(a.total(), 2);
    }

    #[test]
    fn スコープを絞ると一覧はそのグループだけになる() {
        let mut a = app();
        assert_eq!(shown(&a).len(), 5, "既定は All");

        a.set_scope(1);
        assert_eq!(
            shown(&a),
            vec![
                "PhotoScrubberKit",
                "portal-available-checker",
                "LUCA-Skills"
            ]
        );

        a.set_scope(2);
        assert_eq!(shown(&a), vec!["marutope", "受注A"]);

        a.set_scope(0);
        assert_eq!(shown(&a).len(), 5);
    }

    #[test]
    fn スコープと絞り込みは両方効く() {
        let mut a = app();
        a.query = "ru".into(); // ghq は Scrubber、work は marutope に当たる
        a.set_scope(2);
        a.rebuild(true);

        assert_eq!(shown(&a), vec!["marutope", "受注A"], "一覧はスコープ内だけ");
        assert_eq!(
            a.counts,
            vec![1, 2],
            "件数はスコープに関係なく全グループ分を数える"
        );
    }

    #[test]
    fn サイドバーの移動がそのままスコープになる() {
        let mut a = app();
        assert_eq!(a.scope, 0);
        a.focus = Focus::Groups;
        a.step(1);
        assert_eq!((a.group_cursor, a.scope), (1, 1));
        a.step(1);
        assert_eq!((a.group_cursor, a.scope), (2, 2));
        a.step(1);
        assert_eq!((a.group_cursor, a.scope), (2, 2), "末尾で止まる");
        a.step(-5);
        assert_eq!((a.group_cursor, a.scope), (0, 0), "先頭で止まる");
    }

    #[test]
    fn カーソルは一覧の範囲を出ない() {
        let mut a = app();
        a.step_list(100);
        assert_eq!(a.cursor, a.visible.len() - 1);
        a.step_list(-100);
        assert_eq!(a.cursor, 0);

        a.query = "一致しない文字列".into();
        a.rebuild(true);
        assert!(a.visible.is_empty());
        assert_eq!(a.selected_path(), None, "空でも落ちない");
    }
}
