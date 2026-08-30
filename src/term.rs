use std::{fs, fs::File, io, os::fd::FromRawFd};

use ratatui::{
    crossterm::{
        event::{
            self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind,
            KeyModifiers,
        },
        execute,
        terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    },
    prelude::*,
};

use crate::app::{Action, App, Mode};
use crate::ui::draw;

pub fn inherited_tty(readable: bool) -> Option<i32> {
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
pub fn ensure_stdin_is_tty() {
    if unsafe { libc::isatty(libc::STDIN_FILENO) } == 1 {
        return;
    }
    if let Some(fd) = inherited_tty(true) {
        unsafe { libc::dup2(fd, libc::STDIN_FILENO) };
    }
}

/// 画面の出力先。stdout はコマンド置換でパイプに繋がれているので使えない。
pub fn terminal_writer() -> io::Result<File> {
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

pub fn run(app: &mut App) -> io::Result<Option<String>> {
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
