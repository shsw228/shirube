// 設定したソースからディレクトリを集め、選んだパスを stdout に吐く TUI。
// 画面は端末に直接描くので、シェル側は `cd "$(shirube)"` で結果だけ受け取れる。

use std::{
    collections::{HashMap, HashSet},
    env, fs,
    fs::File,
    io::{self, Write},
    os::fd::FromRawFd,
    path::{Path, PathBuf},
    process::Command,
};

use serde::Deserialize;

use ratatui::{
    crossterm::{
        event::{
            self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind,
            KeyModifiers, MouseButton, MouseEventKind,
        },
        execute,
        terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    },
    prelude::*,
    widgets::{Block, Borders, Paragraph},
};
use unicode_width::UnicodeWidthStr;

// 端末の 16 色から取る。256 色の固定値は利用者のテーマとぶつかり、明るい背景では
// 暗いグレーが読めなくなる。Reset は端末の既定色。
const C_TEXT: Color = Color::Reset;
const C_MUTED: Color = Color::DarkGray;
const C_KEY: Color = Color::Blue;
const C_HIT: Color = Color::Yellow;
const C_SEL_BG: Color = Color::Blue;
const C_SEL_FG: Color = Color::White;

/// 前景と背景をまとめて塗り替える。個々の色を残すと背景に沈む文字が出る。
fn selected_style() -> Style {
    Style::new()
        .bg(C_SEL_BG)
        .fg(C_SEL_FG)
        .add_modifier(Modifier::BOLD)
}

/// 選択行では前景色を指定し直さないと、青地に青文字で沈む。
fn marker_style(selected: bool) -> Style {
    if selected {
        selected_style()
    } else {
        Style::new().fg(C_KEY).add_modifier(Modifier::BOLD)
    }
}

/// 色に加えて下線も引く。単色端末や選択行の上でも見つかるように。
fn hit_style(selected: bool) -> Style {
    let base = if selected {
        Style::new().bg(C_SEL_BG)
    } else {
        Style::new()
    };
    base.fg(C_HIT)
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
}

// ---------------------------------------------------------------- 候補の収集

struct Entry {
    path: String,
    group: usize,
    owner: String,
    repo: String,
}

/// パス末尾 2 階層を owner/repo として切り出す。ghq の表示に合わせている。
fn split_tail(path: &str) -> (String, String) {
    let parts: Vec<&str> = path.trim_end_matches('/').split('/').collect();
    let repo = parts.last().copied().unwrap_or(path).to_string();
    let owner = if parts.len() >= 2 {
        parts[parts.len() - 2].to_string()
    } else {
        repo.clone()
    };
    (owner, repo)
}

#[derive(Debug, Deserialize)]
struct Config {
    #[serde(default)]
    source: Vec<SourceSpec>,
    #[serde(default)]
    history: History,
}

#[derive(Debug, Deserialize)]
struct History {
    #[serde(default = "yes")]
    enabled: bool,
    /// 選択時に走らせるコマンド。{} が選んだパスに置き換わり、
    /// 環境変数 SHIRUBE_PATH にも同じ値が入る。シェルを挟むときは後者を使う。
    /// 例: ["zoxide", "add", "{}"]
    record: Option<Vec<String>>,
    /// 優先度の高い順にパスを 1 行 1 件で吐くコマンド。
    /// 例: ["zoxide", "query", "--list"]
    rank: Option<Vec<String>>,
}

fn yes() -> bool {
    true
}

impl Default for History {
    fn default() -> Self {
        History {
            enabled: true,
            record: None,
            rank: None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct SourceSpec {
    label: String,
    /// パスを 1 行 1 件で吐くコマンド。
    command: Option<Vec<String>>,
    path: Option<String>,
    /// path から数える階層数。root 自身は含めない。
    #[serde(default = "default_depth")]
    depth: usize,
}

fn default_depth() -> usize {
    1
}

impl Default for Config {
    fn default() -> Self {
        Config {
            source: vec![SourceSpec {
                label: "ghq".into(),
                command: Some(vec!["ghq".into(), "list".into(), "--full-path".into()]),
                path: None,
                depth: 1,
            }],
            history: History::default(),
        }
    }
}

fn config_dir() -> PathBuf {
    if let Ok(x) = env::var("XDG_CONFIG_HOME") {
        if !x.is_empty() {
            return PathBuf::from(x);
        }
    }
    PathBuf::from(env::var("HOME").unwrap_or_default()).join(".config")
}

fn expand_home(raw: &str) -> PathBuf {
    match raw.strip_prefix("~/") {
        Some(tail) => PathBuf::from(env::var("HOME").unwrap_or_default()).join(tail),
        None => PathBuf::from(raw),
    }
}

fn load_config(explicit: Option<&str>) -> Result<Config, String> {
    let path = match explicit {
        Some(p) => expand_home(p),
        None => config_dir().join("shirube/config.toml"),
    };
    match fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
        // 明示的に指定された設定が読めないのは黙って流さない。
        Err(e) if explicit.is_some() => Err(format!("{}: {e}", path.display())),
        Err(_) => Ok(Config::default()),
    }
}

fn run_command(cmd: &[String]) -> Vec<String> {
    let Some((program, args)) = cmd.split_first() else {
        return Vec::new();
    };
    let Ok(out) = Command::new(program).args(args).output() else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// ディレクトリを走査する。DirEntry::file_type() はリンクを辿らないので、
/// それで判定するとシンボリックリンクのディレクトリを丸ごと取りこぼす。
/// 実体を見るかわりに、辿った先を覚えて循環で止まらないようにする。
fn walk(
    dir: &Path,
    max_depth: usize,
    depth: usize,
    out: &mut Vec<String>,
    seen: &mut HashSet<PathBuf>,
) {
    if depth > max_depth {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else { return };
    for ent in rd.flatten() {
        if ent.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let path = ent.path();
        if !fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false) {
            continue;
        }
        // リンクが自分より上を指していると無限に潜るので、実体で重複を弾く。
        let real = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if !seen.insert(real) {
            continue;
        }
        out.push(path.to_string_lossy().into_owned());
        walk(&path, max_depth, depth + 1, out, seen);
    }
}

/// パイプで渡されたパスを読む。端末を掴み直す前に呼ぶ必要がある。
fn read_piped_stdin() -> Vec<String> {
    if unsafe { libc::isatty(libc::STDIN_FILENO) } == 1 {
        return Vec::new();
    }
    use std::io::Read as _;
    let mut buf = String::new();
    if io::stdin().read_to_string(&mut buf).is_err() {
        return Vec::new();
    }
    buf.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

fn state_dir() -> PathBuf {
    if let Ok(x) = env::var("XDG_STATE_HOME") {
        if !x.is_empty() {
            return PathBuf::from(x);
        }
    }
    PathBuf::from(env::var("HOME").unwrap_or_default()).join(".local/state")
}

fn history_path() -> PathBuf {
    state_dir().join("shirube/history")
}

struct Visit {
    count: u32,
    last: u64,
    path: String,
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 重み付けは zoxide に倣う。
fn frecency(v: &Visit, now: u64) -> f64 {
    let age = now.saturating_sub(v.last);
    let weight = match age {
        0..=3_600 => 4.0,
        3_601..=86_400 => 2.0,
        86_401..=604_800 => 0.5,
        _ => 0.25,
    };
    v.count as f64 * weight
}

fn read_visits() -> Vec<Visit> {
    let Ok(text) = fs::read_to_string(history_path()) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let mut it = line.splitn(3, '\t');
            let count = it.next()?.parse().ok()?;
            let last = it.next()?.parse().ok()?;
            let path = it.next()?.to_string();
            Some(Visit { count, last, path })
        })
        .collect()
}

const HISTORY_LIMIT: usize = 2000;

fn write_visits(mut visits: Vec<Visit>) {
    if visits.len() > HISTORY_LIMIT {
        let t = now();
        visits.sort_by(|a, b| {
            frecency(b, t)
                .partial_cmp(&frecency(a, t))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        visits.truncate(HISTORY_LIMIT);
    }
    let body: String = visits
        .iter()
        .map(|v| format!("{}\t{}\t{}\n", v.count, v.last, v.path))
        .collect();
    let path = history_path();
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::write(path, body);
}

/// パスから順位への表。値が小さいほど優先。
fn ranked_paths(history: &History) -> HashMap<String, usize> {
    if !history.enabled {
        return HashMap::new();
    }
    let ordered: Vec<String> = match &history.rank {
        Some(cmd) => run_command(cmd),
        None => {
            let mut visits = read_visits();
            let t = now();
            visits.sort_by(|a, b| {
                frecency(b, t)
                    .partial_cmp(&frecency(a, t))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            visits.into_iter().map(|v| v.path).collect()
        }
    };
    ordered
        .into_iter()
        .enumerate()
        .map(|(i, p)| (p, i))
        .collect()
}

fn record_choice(history: &History, path: &str) {
    if !history.enabled {
        return;
    }
    if let Some(cmd) = &history.record {
        let args: Vec<String> = cmd.iter().map(|a| a.replace("{}", path)).collect();
        if let Some((program, rest)) = args.split_first() {
            let _ = Command::new(program)
                .args(rest)
                // {} をそのまま埋めると、シェルを挟んだとき空白入りのパスが割れる。
                // 環境変数でも渡しておけば sh -c 側で正しく引用できる。
                .env("SHIRUBE_PATH", path)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
        return;
    }

    // 自前の台帳は自前ランカーに食わせるためだけに在る。並び順を外へ委ねているなら
    // 誰も読まないので書かない。書くと zoxide 側と二重に記録することにもなる。
    if history.rank.is_some() {
        return;
    }

    let mut visits = read_visits();
    match visits.iter_mut().find(|v| v.path == path) {
        Some(v) => {
            v.count += 1;
            v.last = now();
        }
        None => visits.push(Visit {
            count: 1,
            last: now(),
            path: path.to_string(),
        }),
    }
    write_visits(visits);
}

fn collect(config: &Config, piped: Vec<String>) -> (Vec<String>, Vec<Entry>) {
    let mut groups: Vec<String> = Vec::new();
    let mut entries = Vec::new();

    let add =
        |groups: &mut Vec<String>, entries: &mut Vec<Entry>, label: &str, paths: Vec<String>| {
            if paths.is_empty() {
                return;
            }
            let group = groups.len();
            groups.push(label.to_string());
            for path in paths {
                let (owner, repo) = split_tail(&path);
                entries.push(Entry {
                    path,
                    group,
                    owner,
                    repo,
                });
            }
        };

    let ranks = ranked_paths(&config.history);
    add(&mut groups, &mut entries, "stdin", piped);

    for spec in &config.source {
        let mut paths = match (&spec.command, &spec.path) {
            (Some(cmd), _) => run_command(cmd),
            (None, Some(raw)) => {
                let root = expand_home(raw);
                let mut found = Vec::new();
                if root.is_dir() {
                    let mut seen = HashSet::new();
                    seen.insert(fs::canonicalize(&root).unwrap_or_else(|_| root.clone()));
                    walk(&root, spec.depth, 1, &mut found, &mut seen);
                }
                found.sort();
                found
            }
            (None, None) => Vec::new(),
        };
        paths.dedup();
        if !ranks.is_empty() {
            paths.sort_by_key(|p| ranks.get(p).copied().unwrap_or(usize::MAX));
        }
        add(&mut groups, &mut entries, &spec.label, paths);
    }

    (groups, entries)
}

// ------------------------------------------------------------------ マッチ

/// 出現位置をバイト範囲で返す。char 境界で走査するので日本語でも壊れない。
fn find_all(hay: &str, needle: &str, ci: bool) -> Vec<(usize, usize)> {
    if needle.is_empty() {
        return Vec::new();
    }
    let h: Vec<(usize, char)> = hay.char_indices().collect();
    let n: Vec<char> = needle.chars().collect();
    let norm = |c: char| {
        if ci {
            c.to_lowercase().next().unwrap_or(c)
        } else {
            c
        }
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i + n.len() <= h.len() {
        if (0..n.len()).all(|k| norm(h[i + k].1) == norm(n[k])) {
            let (start, _) = h[i];
            let (last_off, last_ch) = h[i + n.len() - 1];
            out.push((start, last_off + last_ch.len_utf8()));
            i += n.len();
        } else {
            i += 1;
        }
    }
    out
}

/// 一致範囲で色を分けた Span 列を作る。範囲は重ならない前提（find_all がそう返す）。
fn highlight(text: &str, ranges: &[(usize, usize)], base: Style, hit: Style) -> Vec<Span<'static>> {
    if ranges.is_empty() {
        return vec![Span::styled(text.to_string(), base)];
    }
    let mut spans = Vec::new();
    let mut pos = 0;
    for &(s, e) in ranges {
        if s > pos {
            spans.push(Span::styled(text[pos..s].to_string(), base));
        }
        spans.push(Span::styled(text[s..e].to_string(), hit));
        pos = e;
    }
    if pos < text.len() {
        spans.push(Span::styled(text[pos..].to_string(), base));
    }
    spans
}

fn merged_ranges(text: &str, tokens: &[String], ci: bool) -> Vec<(usize, usize)> {
    let mut all: Vec<(usize, usize)> = tokens.iter().flat_map(|t| find_all(text, t, ci)).collect();
    all.sort();
    let mut out: Vec<(usize, usize)> = Vec::new();
    for r in all {
        match out.last_mut() {
            Some(last) if r.0 <= last.1 => last.1 = last.1.max(r.1),
            _ => out.push(r),
        }
    }
    out
}

// -------------------------------------------------------------------- 状態

#[derive(PartialEq)]
enum Mode {
    Normal,
    Search,
}

#[derive(PartialEq, Debug)]
enum Action {
    None,
    Quit,
    Choose,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Focus {
    Groups,
    List,
}

struct App {
    groups: Vec<String>,
    entries: Vec<Entry>,
    query: String,
    tokens: Vec<String>,
    ci: bool,
    mode: Mode,
    focus: Focus,
    scope: usize,
    counts: Vec<usize>,
    visible: Vec<usize>,
    cursor: usize,
    offset: usize,
    group_cursor: usize,
    group_offset: usize,
    owner_width: usize,
    tag_width: usize,
    count: Option<usize>,
    operator: Option<char>,
    view_height: usize,
    list_rect: Rect,
    sidebar_rect: Option<Rect>,
}

impl App {
    fn new(groups: Vec<String>, entries: Vec<Entry>) -> Self {
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

    fn matches(&self, e: &Entry) -> bool {
        self.tokens.iter().all(|t| {
            !find_all(&e.repo, t, self.ci).is_empty()
                || !find_all(&e.owner, t, self.ci).is_empty()
                || !find_all(&self.groups[e.group], t, self.ci).is_empty()
        })
    }

    fn rebuild(&mut self, reset_cursor: bool) {
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

    fn total(&self) -> usize {
        self.counts.iter().sum()
    }

    fn group_rows(&self) -> usize {
        self.groups.len() + 1
    }

    fn set_scope(&mut self, scope: usize) {
        if self.scope == scope {
            return;
        }
        self.scope = scope;
        self.rebuild(true);
    }

    fn len_focused(&self) -> usize {
        match self.focus {
            Focus::List => self.visible.len(),
            Focus::Groups => self.group_rows(),
        }
    }

    fn cursor_focused(&self) -> usize {
        match self.focus {
            Focus::List => self.cursor,
            Focus::Groups => self.group_cursor,
        }
    }

    fn goto(&mut self, index: isize) {
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

    fn step_list(&mut self, delta: isize) {
        if self.visible.is_empty() {
            return;
        }
        let last = self.visible.len() as isize - 1;
        self.cursor = (self.cursor as isize + delta).clamp(0, last) as usize;
    }

    fn step(&mut self, delta: isize) {
        self.goto(self.cursor_focused() as isize + delta);
    }

    fn jump(&mut self, to_end: bool) {
        let last = self.len_focused() as isize - 1;
        self.goto(if to_end { last } else { 0 });
    }

    fn jump_screen(&mut self, where_to: char) {
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

    fn scroll_cursor_to(&mut self, where_to: char) {
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

    fn scroll_view(&mut self, delta: isize) {
        if self.focus != Focus::List || self.visible.is_empty() {
            return;
        }
        let last = self.visible.len().saturating_sub(1);
        self.offset = (self.offset as isize + delta).clamp(0, last as isize) as usize;
        let bottom = self.offset + self.view_height.saturating_sub(1);
        self.cursor = self.cursor.clamp(self.offset, bottom.min(last));
    }

    fn jump_group_edge(&mut self, forward: bool) {
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

    fn selected(&self) -> Option<&Entry> {
        self.visible.get(self.cursor).map(|&i| &self.entries[i])
    }

    fn selected_path(&self) -> Option<&str> {
        self.selected().map(|e| e.path.as_str())
    }

    fn take_count(&mut self) -> usize {
        self.count.take().unwrap_or(1)
    }

    fn clear_pending(&mut self) {
        self.count = None;
        self.operator = None;
    }

    fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Groups => Focus::List,
            Focus::List => Focus::Groups,
        };
    }

    fn on_normal_key(&mut self, code: KeyCode, ctrl: bool) -> Action {
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
                ('z', KeyCode::Char(c @ ('z' | 't' | 'b'))) => self.scroll_cursor_to(c),
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
        let page = self.view_height.max(1);

        match code {
            KeyCode::Char('q') => return Action::Quit,
            KeyCode::Esc => {
                if self.query.is_empty() {
                    return Action::Quit;
                }
                self.query.clear();
                self.rebuild(true);
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

            KeyCode::Char('e') if ctrl => self.scroll_view(count as isize),
            KeyCode::Char('y') if ctrl => self.scroll_view(-(count as isize)),

            KeyCode::Char(c @ ('H' | 'M' | 'L')) => self.jump_screen(c),
            KeyCode::Char('}') => self.jump_group_edge(true),
            KeyCode::Char('{') => self.jump_group_edge(false),

            KeyCode::Char('h') | KeyCode::Left => self.focus = Focus::Groups,
            KeyCode::Char('l') | KeyCode::Right => self.focus = Focus::List,
            KeyCode::Tab | KeyCode::BackTab => self.toggle_focus(),
            _ => {}
        }
        Action::None
    }

    fn on_mouse(&mut self, kind: MouseEventKind, col: u16, row: u16) -> Action {
        let in_rect =
            |r: Rect| col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height;
        let over_sidebar = self.sidebar_rect.is_some_and(in_rect);
        let over_list = in_rect(self.list_rect);

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
                    let rect = self.sidebar_rect.unwrap();
                    let i = self.group_offset + (row - rect.y) as usize;
                    if i < self.group_rows() {
                        self.focus = Focus::Groups;
                        self.goto(i as isize);
                    }
                } else if over_list {
                    let i = self.offset + (row - self.list_rect.y) as usize;
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

    fn on_search_key(&mut self, code: KeyCode, ctrl: bool) -> Action {
        match code {
            KeyCode::Enter => self.mode = Mode::Normal,
            KeyCode::Esc => {
                self.query.clear();
                self.mode = Mode::Normal;
                self.rebuild(true);
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.rebuild(true);
            }
            KeyCode::Char('u') if ctrl => {
                self.query.clear();
                self.rebuild(true);
            }
            KeyCode::Char('w') if ctrl => {
                while self.query.ends_with(' ') {
                    self.query.pop();
                }
                while !self.query.is_empty() && !self.query.ends_with(' ') {
                    self.query.pop();
                }
                self.rebuild(true);
            }
            KeyCode::Up => self.step_list(-1),
            KeyCode::Down => self.step_list(1),
            KeyCode::Char('p') if ctrl => self.step_list(-1),
            KeyCode::Char('n') if ctrl => self.step_list(1),
            KeyCode::Char(c) => {
                self.query.push(c);
                self.rebuild(true);
            }
            _ => {}
        }
        Action::None
    }
}

// ------------------------------------------------------------------ 描画

struct Panes {
    sidebar: Option<Rect>,
    list: Rect,
    preview: Option<Rect>,
}

fn split_panes(area: Rect, sidebar_width: u16) -> Panes {
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

fn pane(title: &str, focused: bool) -> Block<'static> {
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

fn sidebar_width(app: &App) -> u16 {
    let label = app
        .groups
        .iter()
        .map(|g| g.width())
        .chain(std::iter::once("All".width()))
        .max()
        .unwrap_or(3);
    ((label + 10) as u16).clamp(14, 26)
}

fn sidebar_lines(app: &mut App, inner: Rect) -> Vec<Line<'static>> {
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

fn entry_line(app: &App, idx: usize, selected: bool) -> Line<'static> {
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

fn preview_lines(path: &str, max: usize) -> Vec<Line<'static>> {
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

fn shorten_home(path: &str) -> String {
    match env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&home) => {
            format!("~{}", &path[home.len()..])
        }
        _ => path.to_string(),
    }
}

fn pending_text(app: &App) -> String {
    let mut out = String::new();
    if let Some(n) = app.count {
        out.push_str(&n.to_string());
    }
    if let Some(op) = app.operator {
        out.push(op);
    }
    out
}

fn status_line(app: &App) -> Line<'static> {
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

fn path_line(path: &str) -> Line<'static> {
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

fn draw(f: &mut Frame, app: &mut App) {
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

// -------------------------------------------------------------------- 本体

fn inherited_tty(readable: bool) -> Option<i32> {
    for fd in [libc::STDERR_FILENO, libc::STDOUT_FILENO, libc::STDIN_FILENO] {
        if unsafe { libc::isatty(fd) } != 1 {
            continue;
        }
        if readable {
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            if flags < 0 || (flags & libc::O_ACCMODE) == libc::O_WRONLY {
                continue;
            }
        }
        return Some(fd);
    }
    None
}

/// crossterm は stdin が tty ならそれを、そうでなければ /dev/tty を開いて入力に使う。
/// ところが macOS では新しく開いた /dev/tty を kqueue に登録できず (EINVAL)、
/// イベント読み取りの初期化ごと失敗する。zsh の zle は widget から起動した
/// コマンドの stdin を tty から外すので、この経路にそのまま嵌る。
/// 継承済みの端末 fd なら登録できるため、それを fd 0 に被せてから crossterm に渡す。
fn ensure_stdin_is_tty() {
    if unsafe { libc::isatty(libc::STDIN_FILENO) } == 1 {
        return;
    }
    if let Some(fd) = inherited_tty(true) {
        unsafe { libc::dup2(fd, libc::STDIN_FILENO) };
    }
}

/// 画面の出力先。stdout はコマンド置換でパイプに繋がれているので使えない。
fn terminal_writer() -> io::Result<File> {
    if let Some(fd) = inherited_tty(false) {
        let dup = unsafe { libc::dup(fd) };
        if dup >= 0 {
            return Ok(unsafe { File::from_raw_fd(dup) });
        }
    }
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
}

fn run(app: &mut App) -> io::Result<Option<String>> {
    ensure_stdin_is_tty();
    let mut out = terminal_writer()?;

    enable_raw_mode()?;
    execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
    let mut term = Terminal::new(CrosstermBackend::new(out))?;

    let result = loop {
        term.draw(|f| draw(f, app))?;

        let key = match event::read()? {
            Event::Key(k) if k.kind == KeyEventKind::Press => k,
            Event::Mouse(m) => {
                if app.on_mouse(m.kind, m.column, m.row) == Action::Choose {
                    if let Some(p) = app.selected_path() {
                        break Some(p.to_string());
                    }
                }
                continue;
            }
            _ => continue,
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        if ctrl && matches!(key.code, KeyCode::Char('c')) {
            break None;
        }

        let action = match app.mode {
            Mode::Normal => app.on_normal_key(key.code, ctrl),
            Mode::Search => app.on_search_key(key.code, ctrl),
        };
        match action {
            Action::Quit => break None,
            Action::Choose => {
                if let Some(p) = app.selected_path() {
                    break Some(p.to_string());
                }
            }
            Action::None => {}
        }
    };

    disable_raw_mode()?;
    execute!(
        term.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    term.show_cursor()?;
    Ok(result)
}

const HELP: &str = "\
shirube — a directory jumper for the shell

Usage: shirube [OPTIONS] [QUERY]
       <command producing paths> | shirube

Sources are declared in $XDG_CONFIG_HOME/shirube/config.toml. Paths piped on
stdin are listed as a group of their own. The chosen directory is printed to
stdout.

Options:
  -q, --query TEXT    Start with this filter applied
      --config PATH   Use this config file instead of the default
  -h, --help          Print this help
  -V, --version       Print the version

Keys (vim motions, with counts such as 5j and 12G):
  j k               move             gg G      first / last
  h l Tab           switch pane      { }       group boundary
  Ctrl-d Ctrl-u     half page        H M L     top / middle / bottom
  Ctrl-f Ctrl-b     page             zz zt zb  reposition
  Ctrl-e Ctrl-y     scroll view      /         search
  Enter             choose           q         quit

The mouse works too: the wheel scrolls the pane under the pointer, a click
selects a row, and clicking the selected row again chooses it.
";

fn main() {
    let mut args = env::args().skip(1);
    let mut config_path: Option<String> = None;
    let mut query = String::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                return;
            }
            "-V" | "--version" => {
                println!("shirube {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            "--config" => match args.next() {
                Some(p) => config_path = Some(p),
                None => {
                    eprintln!("shirube: --config needs a path");
                    std::process::exit(2);
                }
            },
            "--query" | "-q" => match args.next() {
                Some(q) => query = q,
                None => {
                    eprintln!("shirube: --query needs a value");
                    std::process::exit(2);
                }
            },
            other if other.starts_with('-') => {
                eprintln!("shirube: unknown argument: {other}");
                std::process::exit(2);
            }
            other => query = other.to_string(),
        }
    }

    let config = match load_config(config_path.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("shirube: {e}");
            std::process::exit(2);
        }
    };

    let piped = read_piped_stdin();

    let (groups, entries) = collect(&config, piped);
    if entries.is_empty() {
        eprintln!("shirube: no directories to show");
        std::process::exit(1);
    }

    let mut app = App::new(groups, entries);
    if !query.is_empty() {
        app.query = query;
        app.rebuild(true);
    }
    match run(&mut app) {
        Ok(Some(path)) => {
            record_choice(&config.history, &path);
            let mut stdout = io::stdout();
            let _ = writeln!(stdout, "{path}");
        }
        Ok(None) => std::process::exit(1),
        Err(e) => {
            let _ = disable_raw_mode();
            eprintln!("shirube: {e}");
            std::process::exit(1);
        }
    }
}

// -------------------------------------------------------------------- テスト

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;

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
    fn 部分文字列でのみ一致する() {
        assert_eq!(find_all("PhotoScrubberKit", "work", true), vec![]);
        assert_eq!(find_all("marutope", "maru", true), vec![(0, 4)]);
    }

    #[test]
    fn スマートケースで大文字は絞り込みを強める() {
        assert_eq!(find_all("GitGrass", "git", true), vec![(0, 3)]);
        assert_eq!(find_all("GitGrass", "git", false), vec![]);
    }

    #[test]
    fn 日本語でもバイト境界が壊れない() {
        let hits = find_all("marutope/受注A", "受注", true);
        assert_eq!(hits.len(), 1);
        let (s, e) = hits[0];
        assert_eq!(&"marutope/受注A"[s..e], "受注");
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

    fn key(a: &mut App, k: char) -> Action {
        a.on_normal_key(KeyCode::Char(k), false)
    }

    fn keys(a: &mut App, s: &str) {
        for c in s.chars() {
            key(a, c);
        }
    }

    fn big() -> App {
        let entries = (0..30)
            .map(|i| entry(if i < 20 { 0 } else { 1 }, &format!("/g/own{i}/repo{i}")))
            .collect();
        let mut a = App::new(vec!["ghq".into(), "work".into()], entries);
        a.view_height = 10;
        a
    }

    #[test]
    fn ggとGで端に飛ぶ() {
        let mut a = big();
        keys(&mut a, "G");
        assert_eq!(a.cursor, 29);
        keys(&mut a, "gg");
        assert_eq!(a.cursor, 0);
    }

    #[test]
    fn gは一打鍵では動かない() {
        let mut a = big();
        keys(&mut a, "G");
        keys(&mut a, "g");
        assert_eq!(a.cursor, 29, "g だけでは確定しない");
        assert_eq!(a.operator, Some('g'));
        keys(&mut a, "g");
        assert_eq!(a.cursor, 0);
        assert_eq!(a.operator, None);
    }

    #[test]
    fn 数値プレフィックスが効く() {
        let mut a = big();
        keys(&mut a, "5j");
        assert_eq!(a.cursor, 5);
        keys(&mut a, "12G");
        assert_eq!(a.cursor, 11, "12G は 12 行目");
        keys(&mut a, "3gg");
        assert_eq!(a.cursor, 2, "3gg も行指定");
        keys(&mut a, "2k");
        assert_eq!(a.cursor, 0, "端で止まる");
    }

    #[test]
    fn Escで打ちかけの入力を取り消す() {
        let mut a = big();
        keys(&mut a, "5");
        assert_eq!(a.count, Some(5));
        assert_eq!(a.on_normal_key(KeyCode::Esc, false), Action::None);
        assert_eq!(a.count, None, "終了ではなく取り消し");
        keys(&mut a, "j");
        assert_eq!(a.cursor, 1, "取り消した 5 は残らない");
    }

    #[test]
    fn HMLは画面内の位置へ飛ぶ() {
        let mut a = big();
        a.offset = 10;
        a.cursor = 12;
        keys(&mut a, "H");
        assert_eq!(a.cursor, 10);
        keys(&mut a, "L");
        assert_eq!(a.cursor, 19);
        keys(&mut a, "M");
        assert_eq!(a.cursor, 14);
    }

    #[test]
    fn zzztzbで表示位置を変える() {
        let mut a = big();
        a.cursor = 20;
        keys(&mut a, "zt");
        assert_eq!(a.offset, 20);
        keys(&mut a, "zb");
        assert_eq!(a.offset, 11);
        keys(&mut a, "zz");
        assert_eq!(a.offset, 15);
    }

    #[test]
    fn 波括弧でグループの切れ目へ飛ぶ() {
        let mut a = big();
        keys(&mut a, "}");
        assert_eq!(a.cursor, 20, "次のグループの先頭");
        keys(&mut a, "}");
        assert_eq!(a.cursor, 29, "先が無ければ末尾");
        keys(&mut a, "{");
        assert_eq!(a.cursor, 20, "まず今のグループの先頭");
        keys(&mut a, "{");
        assert_eq!(a.cursor, 0, "次に前のグループの先頭");
    }

    #[test]
    fn ページ移動は一覧の高さに従う() {
        let mut a = big();
        a.on_normal_key(KeyCode::Char('d'), true);
        assert_eq!(a.cursor, 5, "Ctrl-d は半ページ");
        a.on_normal_key(KeyCode::Char('f'), true);
        assert_eq!(a.cursor, 15, "Ctrl-f は 1 ページ");
        a.on_normal_key(KeyCode::Char('b'), true);
        assert_eq!(a.cursor, 5);
    }

    #[test]
    fn qとEnterの結果が呼び出し側に返る() {
        let mut a = big();
        assert_eq!(a.on_normal_key(KeyCode::Enter, false), Action::Choose);
        assert_eq!(key(&mut a, 'q'), Action::Quit);

        a.focus = Focus::Groups;
        assert_eq!(a.on_normal_key(KeyCode::Enter, false), Action::None);
        assert_eq!(a.focus, Focus::List, "サイドバーからは一覧へ移るだけ");
    }

    #[test]
    fn 設定が無ければghqだけを見る() {
        let c = Config::default();
        assert_eq!(c.source.len(), 1);
        assert_eq!(c.source[0].label, "ghq");
    }

    #[test]
    fn 設定はコマンドとディレクトリの両方を受け取る() {
        let c: Config = toml::from_str(
            r#"
            [[source]]
            label = "ghq"
            command = ["ghq", "list", "--full-path"]

            [[source]]
            label = "work"
            path = "~/Developer/work"
            depth = 2

            [[source]]
            label = "notes"
            path = "/tmp/notes"
            "#,
        )
        .expect("設定を読めること");

        assert_eq!(
            c.source
                .iter()
                .map(|s| s.label.as_str())
                .collect::<Vec<_>>(),
            ["ghq", "work", "notes"]
        );
        assert_eq!(c.source[1].depth, 2);
        assert_eq!(c.source[2].depth, 1, "depth は省略できる");
    }

    #[test]
    fn コマンドの出力がグループになる() {
        let c: Config = toml::from_str(
            r#"
            [[source]]
            label = "fixed"
            command = ["printf", "/tmp/a\n/tmp/b\n"]
            "#,
        )
        .unwrap();
        let (groups, entries) = collect(&c, Vec::new());
        assert_eq!(groups, ["fixed"]);
        assert_eq!(
            entries.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(),
            ["/tmp/a", "/tmp/b"]
        );
    }

    #[test]
    fn パイプ入力は独立したグループになる() {
        let c = Config {
            source: Vec::new(),
            history: History {
                enabled: false,
                ..Default::default()
            },
        };
        let (groups, entries) = collect(&c, vec!["/tmp/x".into(), "/tmp/y".into()]);
        assert_eq!(groups, ["stdin"]);
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| e.group == 0));
    }

    #[test]
    fn シンボリックリンクのディレクトリも拾う() {
        let base = std::env::temp_dir().join(format!("shirube-sym-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("root/real")).unwrap();
        fs::create_dir_all(base.join("outside/inner")).unwrap();
        std::os::unix::fs::symlink(base.join("outside"), base.join("root/linked")).unwrap();

        let mut found = Vec::new();
        let mut seen = HashSet::new();
        walk(&base.join("root"), 2, 1, &mut found, &mut seen);

        let names: Vec<String> = found.iter().map(|p| split_tail(p).1).collect();
        assert!(names.contains(&"real".to_string()));
        assert!(
            names.contains(&"linked".to_string()),
            "リンクを取りこぼした"
        );
        assert!(
            names.contains(&"inner".to_string()),
            "リンクの先まで辿れていない"
        );

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn リンクの循環で止まらない() {
        let base = std::env::temp_dir().join(format!("shirube-loop-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("root/a")).unwrap();
        std::os::unix::fs::symlink(base.join("root"), base.join("root/a/back")).unwrap();

        let mut found = Vec::new();
        let mut seen = HashSet::new();
        seen.insert(fs::canonicalize(base.join("root")).unwrap());
        walk(&base.join("root"), 10, 1, &mut found, &mut seen);

        assert!(found.len() < 10, "循環を辿り続けている: {found:?}");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn 履歴はzoxideなどに委譲できる() {
        let c: Config = toml::from_str(
            r#"
            [history]
            record = ["zoxide", "add", "{}"]
            rank = ["zoxide", "query", "--list"]
            "#,
        )
        .expect("設定を読めること");

        assert!(c.history.enabled, "既定で有効");
        assert_eq!(
            c.history.record.as_deref(),
            Some(["zoxide".to_string(), "add".into(), "{}".into()].as_slice())
        );
        assert_eq!(
            c.history.rank.as_deref(),
            Some(["zoxide".to_string(), "query".into(), "--list".into()].as_slice())
        );
    }

    #[test]
    fn recordは任意のコマンドを実行できる() {
        let out = std::env::temp_dir().join(format!("shirube-rec-{}", std::process::id()));
        let _ = fs::remove_file(&out);
        let target = "/tmp/dir with space/x";

        let h = History {
            enabled: true,
            record: Some(vec![
                "sh".into(),
                "-c".into(),
                format!("printf %s \"$1\" > {} ", out.display()),
                "sh".into(),
                "{}".into(),
            ]),
            rank: None,
        };
        record_choice(&h, target);
        assert_eq!(fs::read_to_string(&out).unwrap(), target);
        let _ = fs::remove_file(&out);
    }

    #[test]
    fn recordは環境変数でもパスを渡す() {
        let out = std::env::temp_dir().join(format!("shirube-env-{}", std::process::id()));
        let _ = fs::remove_file(&out);
        let target = "/tmp/dir with space/y";

        let h = History {
            enabled: true,
            record: Some(vec![
                "sh".into(),
                "-c".into(),
                format!("printf %s \"$SHIRUBE_PATH\" > {}", out.display()),
            ]),
            rank: None,
        };
        record_choice(&h, target);
        assert_eq!(fs::read_to_string(&out).unwrap(), target);
        let _ = fs::remove_file(&out);
    }

    #[test]
    fn rankを委譲したら自前の台帳は書かない() {
        let dir = std::env::temp_dir().join(format!("shirube-state-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        unsafe { std::env::set_var("XDG_STATE_HOME", &dir) };

        let delegated = History {
            enabled: true,
            record: None,
            rank: Some(vec!["true".into()]),
        };
        record_choice(&delegated, "/tmp/x");
        assert!(!history_path().exists(), "委譲時に台帳を書いている");

        let builtin = History {
            enabled: true,
            record: None,
            rank: None,
        };
        record_choice(&builtin, "/tmp/x");
        assert!(history_path().exists(), "既定では台帳を書く");

        unsafe { std::env::remove_var("XDG_STATE_HOME") };
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 履歴の設定が無ければ自前の台帳を使う() {
        let c = Config::default();
        assert!(c.history.enabled);
        assert!(c.history.record.is_none());
        assert!(c.history.rank.is_none());
    }

    #[test]
    fn 順位コマンドの並びが一覧に反映される() {
        let c: Config = toml::from_str(
            r#"
            [[source]]
            label = "s"
            command = ["printf", "/tmp/a\n/tmp/b\n/tmp/c\n"]

            [history]
            rank = ["printf", "/tmp/c\n/tmp/a\n"]
            "#,
        )
        .unwrap();
        let (_, entries) = collect(&c, Vec::new());
        assert_eq!(
            entries.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(),
            ["/tmp/c", "/tmp/a", "/tmp/b"],
            "順位付きが前、知らないものは元の並びで後ろ"
        );
    }

    #[test]
    fn 履歴を切ると並べ替えない() {
        let c: Config = toml::from_str(
            r#"
            [[source]]
            label = "s"
            command = ["printf", "/tmp/a\n/tmp/b\n/tmp/c\n"]

            [history]
            enabled = false
            rank = ["printf", "/tmp/c\n"]
            "#,
        )
        .unwrap();
        let (_, entries) = collect(&c, Vec::new());
        assert_eq!(
            entries.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(),
            ["/tmp/a", "/tmp/b", "/tmp/c"]
        );
    }

    #[test]
    fn frecencyは新しいほど重い() {
        let t = 10_000_000;
        let fresh = Visit {
            count: 1,
            last: t,
            path: "a".into(),
        };
        let old = Visit {
            count: 3,
            last: t - 60 * 86_400,
            path: "b".into(),
        };
        assert!(
            frecency(&fresh, t) > frecency(&old, t),
            "回数が少なくても直近なら上に来る"
        );
    }

    #[test]
    fn 空のソースは見出しごと出さない() {
        let c: Config = toml::from_str(
            r#"
            [[source]]
            label = "empty"
            path = "/nonexistent-path-for-test"
            "#,
        )
        .unwrap();
        let (groups, entries) = collect(&c, Vec::new());
        assert!(groups.is_empty());
        assert!(entries.is_empty());
    }
}

#[cfg(test)]
#[allow(non_snake_case)]
mod render_tests {
    use super::*;
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
