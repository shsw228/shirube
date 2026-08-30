# shirube

[![CI](https://github.com/shsw228/shirube/actions/workflows/ci.yml/badge.svg)](https://github.com/shsw228/shirube/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/shsw228/shirube?sort=semver&label=release)](https://github.com/shsw228/shirube/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A terminal directory jumper that keeps your directories in named groups. It
collects paths from the sources you configure, lets you pick one with vim
motions, and prints it for your shell to `cd` into.

*Shirube* (標) is Japanese for a signpost — the thing that tells you where each
road goes.

```
╭ groups ────╮╭ work · 3 dirs ──────────────────────╮╭ acme-site ───────╮
│  All     60││❯ work      acme-site                ││ docs/            │
│  ghq     57││  acme-site  frontend                ││ frontend/        │
│❯ work     3││  acme-site  backend                 ││ backend/         │
╰────────────╯╰─────────────────────────────────────╯╰──────────────────╯
 ~/Developer/work/acme-site
 j/k move   h/l pane   tab switch   / search   enter go   q quit
```

## Features

- **Groups as a pane, not headings.** Filtering never costs you the sense of
  where you are: every group stays on the left with its own hit count, even
  when nothing in it matches.
- **Substring matching, not fuzzy.** Searching `work` will not return
  `shsw228/PhotoScrubberKit` by scavenging a `w` from the owner and an `o`, `r`
  and `K` from the repository name. Matching is smart-case over what is on
  screen, and every hit is highlighted.
- **Any source.** A command that prints paths, a directory to walk, or paths
  piped on stdin. `ghq` is the default, not a requirement.
- **Any history backend.** Picks are ranked recent-first. Keep the built-in
  ledger, or delegate to zoxide or anything else that can record and rank.
- **vim motions.** Counts, `gg`/`G`, `H`/`M`/`L`, `zz`/`zt`/`zb`, `{`/`}`.
- **Responsive.** Panes are added and dropped with the terminal width.
- **Your colour scheme.** Drawn with the terminal's own 16 colours.

## Requirements

- A terminal that supports 16 colours and ANSI escape sequences
- macOS or Linux
- Optionally [ghq](https://github.com/x-motemen/ghq), used by the default
  configuration

## Installation

### From a release

Download the archive for your platform from
[Releases](https://github.com/shsw228/shirube/releases/latest) and put the
`shirube` binary somewhere on your `PATH`.

Prebuilt binaries are published for `aarch64-apple-darwin`,
`x86_64-apple-darwin`, `x86_64-unknown-linux-gnu` and
`aarch64-unknown-linux-gnu`.

### From source

Requires Rust 1.88 or later (the minimum supported version, checked in CI).

```sh
cargo install --git https://github.com/shsw228/shirube
```

## Usage

```
shirube [OPTIONS] [QUERY]
<command producing paths> | shirube
```

shirube draws to the terminal and prints the chosen directory to stdout. If you
quit without choosing, it prints nothing and exits with status 1.

| Option | Description |
| --- | --- |
| `-q`, `--query TEXT` | Start with this filter applied |
| `--config PATH` | Use this configuration file |
| `-h`, `--help` | Print help |
| `-V`, `--version` | Print the version |

A bare argument is taken as the initial query, so `shirube acme` opens filtered.

## Shell integration

Because the chosen path goes to stdout, the wrapper is one command
substitution.

<details open>
<summary>zsh</summary>

```zsh
function _shirube() {
  local target
  target="$(shirube)"
  if [ -n "${target}" ]; then
    cd "${target}" || return
  fi
  zle reset-prompt
}
zle -N _shirube
bindkey '^g' _shirube
```

</details>

<details>
<summary>bash</summary>

```bash
bind '"\C-g": "\C-a\C-k cd \"$(shirube)\"\C-m"'
```

</details>

<details>
<summary>fish</summary>

```fish
function _shirube
    set -l target (shirube)
    test -n "$target"; and cd $target
    commandline -f repaint
end
bind \cg _shirube
```

</details>

## Configuration

shirube reads `$XDG_CONFIG_HOME/shirube/config.toml`, defaulting to
`~/.config/shirube/config.toml`. Without a configuration file it lists ghq
repositories and keeps its own history.

### Sources

Each `[[source]]` becomes a group, in the order written. A source is either a
command that prints one path per line, or a directory to walk.

```toml
[[source]]
label = "ghq"
command = ["ghq", "list", "--full-path"]

[[source]]
label = "work"
path = "~/Developer/work"
depth = 2
```

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `label` | string | required | Name shown in the groups pane |
| `command` | array | — | Argv printing one path per line |
| `path` | string | — | Directory to walk; `~/` is expanded |
| `depth` | integer | `1` | Levels below `path`; the root itself is not listed |

Hidden directories are skipped, symlinked directories are followed, and a source
that yields nothing is omitted along with its heading — so listing a tool you
have not installed costs nothing.

Paths piped on stdin become a `stdin` group ahead of the configured sources:

```sh
fd --type d --max-depth 2 . ~/src | shirube
```

### History

Picks are ranked the way zoxide ranks: recent beats frequent. With no
configuration shirube keeps its own ledger at `$XDG_STATE_HOME/shirube/history`.

`record` and `rank` are plain argv, so any tool can be the backend. `record`
runs on selection; `rank` prints paths one per line, best first.

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `enabled` | boolean | `true` | Turn ranking and recording off |
| `record` | array | built-in ledger | Argv run when a directory is chosen |
| `rank` | array | built-in ledger | Argv printing paths in priority order |

If your tool already hooks `cd`, set `rank` alone — its shell integration does
the recording, and adding `record` would count the same jump twice:

```toml
[history]
rank = ["zoxide", "query", "--list"]
```

Setting `rank` alone also stops shirube writing its own ledger: the ledger only
exists to feed the built-in ranker, so once ranking is delegated nothing would
ever read it.

If your backend records nothing on its own, that is what `record` is for:

```toml
[history]
record = ["sh", "-c", 'printf "%s\n" "$SHIRUBE_PATH" >> ~/.local/state/dirs']
rank = ["sh", "-c", 'tail -r ~/.local/state/dirs | awk "!seen[\$0]++"']
```

`record` also earns its keep when the chosen path is not used for `cd` at all —
`vim "$(shirube)"`, opening a tmux window, feeding another script — because then
no `cd` happens for anything to hook.

The chosen path reaches the command two ways: `{}` is substituted into each
argument, and `SHIRUBE_PATH` is set in the environment. Prefer `{}` when you
exec the tool directly and `$SHIRUBE_PATH` when a shell is in between, since
`{}` is not quoted and a path containing spaces would be split. Writing the
shell fragment as a TOML *literal* string (single quotes) keeps its own quoting
intact.

If your ranking tool prints more than a path per line, trim it in the command:

```toml
rank = ["sh", "-c", "zoxide query --list --score | awk '{print $2}'"]
```

## Key bindings

The interface is modal. It opens in normal mode with the list focused.

### Normal mode

| Key | Action |
| --- | --- |
| `j` `k`, `↓` `↑` | Move within the focused pane |
| `h` `l`, `←` `→` | Focus the groups pane / the list |
| `Tab` | Switch panes |
| `gg` `G` | First / last entry |
| `{n}G`, `{n}gg` | Go to entry *n* |
| `{n}j`, `{n}k` | Repeat a motion |
| `Ctrl-d` `Ctrl-u` | Half page |
| `Ctrl-f` `Ctrl-b` | Page |
| `Ctrl-e` `Ctrl-y` | Scroll the view without moving the cursor |
| `H` `M` `L` | Top / middle / bottom of the screen |
| `zz` `zt` `zb` | Put the cursor line at the centre / top / bottom |
| `{` `}` | Previous / next group boundary |
| `/` | Enter search mode |
| `Enter` | Choose, or enter the list from the groups pane |
| `q` | Quit |
| `Esc` | Cancel a pending count or `g`, else clear the filter, else quit |

Moving in the groups pane narrows the list to that group straight away. `All`
shows every group at once, tagged with where each entry came from.

### Search mode

The query line sits at the bottom of the screen. Space-separated words are
ANDed, and a query containing an uppercase letter becomes case-sensitive.

| Key | Action |
| --- | --- |
| `Enter` | Keep the filter and return to normal mode |
| `Esc` | Discard the filter |
| `Ctrl-w` `Ctrl-u` | Delete the last word / the whole query |
| `↑` `↓`, `Ctrl-p` `Ctrl-n` | Move within the list while typing |

### Mouse

The wheel scrolls whichever pane the pointer is over, a click selects a row, and
clicking the row that is already selected chooses it.

## Layout

| Terminal width | Panes |
| --- | --- |
| 100 or wider | groups, list, preview |
| 60 – 99 | groups, list |
| under 60 | list only |

Colours come from the terminal's own 16-colour palette, so shirube inherits
whatever scheme is already configured instead of imposing fixed 256-colour
values that turn unreadable on a light background. Matches are coloured *and*
underlined so they stay findable on a selected row or in a monochrome terminal.

## Implementation notes

The terminal is taken from an inherited file descriptor rather than by opening
`/dev/tty`. zsh detaches stdin from the terminal for commands run inside a zle
widget, and on macOS a freshly opened `/dev/tty` cannot be registered with
kqueue (`EINVAL`), which breaks the usual fallback path.

Directory walks follow symlinks but remember the paths already visited, so a
link pointing back up cannot loop.

## Contributing

Issues and pull requests are welcome.

```sh
cargo test           # unit and rendering tests
cargo fmt --all --check
cargo clippy --all -- -D warnings
```

CI runs the same three on macOS and Linux.

## License

MIT — see [LICENSE](LICENSE).

