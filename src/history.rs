use std::{collections::HashMap, env, fs, path::PathBuf, process::Command};

use crate::config::History;
use crate::source::run_command;

pub fn state_dir() -> PathBuf {
    if let Ok(x) = env::var("XDG_STATE_HOME") {
        if !x.is_empty() {
            return PathBuf::from(x);
        }
    }
    PathBuf::from(env::var("HOME").unwrap_or_default()).join(".local/state")
}

pub fn history_path() -> PathBuf {
    state_dir().join("shirube/history")
}

pub struct Visit {
    pub count: u32,
    pub last: u64,
    pub path: String,
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 重み付けは zoxide に倣う。
pub fn frecency(v: &Visit, now: u64) -> f64 {
    let age = now.saturating_sub(v.last);
    let weight = match age {
        0..=3_600 => 4.0,
        3_601..=86_400 => 2.0,
        86_401..=604_800 => 0.5,
        _ => 0.25,
    };
    v.count as f64 * weight
}

pub fn read_visits() -> Vec<Visit> {
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

pub const HISTORY_LIMIT: usize = 2000;

pub fn write_visits(mut visits: Vec<Visit>) {
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
pub fn ranked_paths(history: &History) -> HashMap<String, usize> {
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

pub fn record_choice(history: &History, path: &str) {
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

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;

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
}
