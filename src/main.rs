// 設定したソースからディレクトリを集め、選んだパスを stdout に吐く TUI。
// 画面は端末に直接描くので、シェル側は `cd "$(shirube)"` で結果だけ受け取れる。

mod app;
mod catalog;
mod config;
mod matcher;
mod model;
mod term;
mod ui;

use std::{
    env,
    io::{self, Write},
};

use ratatui::crossterm::terminal::disable_raw_mode;

use crate::app::App;
use crate::catalog::history::{history_path, ranked_paths, record_choice};
use crate::catalog::source::{collect, read_piped_stdin};
use crate::config::load_config;
use crate::term::run;

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
  f                 substring/fuzzy  Enter     choose
                                     q         quit

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

    let ledger = history_path();
    let ranks = ranked_paths(&config.history, &ledger);
    let (groups, entries) = collect(&config, piped, &ranks);
    if entries.is_empty() {
        eprintln!("shirube: no directories to show");
        std::process::exit(1);
    }

    let mut app = App::new(groups, entries);
    app.match_mode = config.matching.mode;
    if !query.is_empty() {
        app.query = query;
        app.rebuild(true);
    }
    match run(&mut app) {
        Ok(Some(path)) => {
            record_choice(&config.history, &ledger, &path);
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
