use std::{env, fs, path::PathBuf};

use serde::Deserialize;

use crate::matcher::MatchMode;

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub source: Vec<SourceSpec>,
    #[serde(default)]
    pub history: History,
    #[serde(default, rename = "match")]
    pub matching: Matching,
}

/// 既定の一致のとり方。実行中は f で切り替えられる。
#[derive(Debug, Default, Deserialize)]
pub struct Matching {
    #[serde(default)]
    pub mode: MatchMode,
}

#[derive(Debug, Deserialize)]
pub struct History {
    #[serde(default = "yes")]
    pub enabled: bool,
    /// 選択時に走らせるコマンド。{} が選んだパスに置き換わり、
    /// 環境変数 SHIRUBE_PATH にも同じ値が入る。シェルを挟むときは後者を使う。
    /// 例: ["zoxide", "add", "{}"]
    pub record: Option<Vec<String>>,
    /// 優先度の高い順にパスを 1 行 1 件で吐くコマンド。
    /// 例: ["zoxide", "query", "--list"]
    pub rank: Option<Vec<String>>,
}

pub fn yes() -> bool {
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
pub struct SourceSpec {
    pub label: String,
    /// パスを 1 行 1 件で吐くコマンド。
    pub command: Option<Vec<String>>,
    pub path: Option<String>,
    /// path から数える階層数。root 自身は含めない。
    #[serde(default = "default_depth")]
    pub depth: usize,
}

pub fn default_depth() -> usize {
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
            matching: Matching::default(),
        }
    }
}

pub fn config_dir() -> PathBuf {
    if let Ok(x) = env::var("XDG_CONFIG_HOME") {
        if !x.is_empty() {
            return PathBuf::from(x);
        }
    }
    PathBuf::from(env::var("HOME").unwrap_or_default()).join(".config")
}

pub fn expand_home(raw: &str) -> PathBuf {
    match raw.strip_prefix("~/") {
        Some(tail) => PathBuf::from(env::var("HOME").unwrap_or_default()).join(tail),
        None => PathBuf::from(raw),
    }
}

pub fn load_config(explicit: Option<&str>) -> Result<Config, String> {
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

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;

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
    fn 一致のとり方を設定で選べる() {
        let c: Config = toml::from_str("[match]\nmode = \"fuzzy\"").unwrap();
        assert_eq!(c.matching.mode, MatchMode::Fuzzy);
        assert_eq!(Config::default().matching.mode, MatchMode::Substring);
    }

    #[test]
    fn 履歴の設定が無ければ自前の台帳を使う() {
        let c = Config::default();
        assert!(c.history.enabled);
        assert!(c.history.record.is_none());
        assert!(c.history.rank.is_none());
    }
}
