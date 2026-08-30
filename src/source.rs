use std::{
    collections::HashSet,
    fs, io,
    path::{Path, PathBuf},
};

use crate::config::{expand_home, Config};
use crate::exec::run_command;
use crate::model::{split_tail, Entry};

/// ディレクトリを走査する。DirEntry::file_type() はリンクを辿らないので、
/// それで判定するとシンボリックリンクのディレクトリを丸ごと取りこぼす。
/// 実体を見るかわりに、辿った先を覚えて循環で止まらないようにする。
pub fn walk(
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
pub fn read_piped_stdin() -> Vec<String> {
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

pub fn collect(config: &Config, piped: Vec<String>) -> (Vec<String>, Vec<Entry>) {
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
        add(&mut groups, &mut entries, &spec.label, paths);
    }

    (groups, entries)
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;
    use crate::config::History;

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
}
