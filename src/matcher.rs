use serde::Deserialize;

/// 何をもって一致とみなすか。
#[derive(Debug, Clone, Copy, PartialEq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MatchMode {
    /// 打った並びがそのまま含まれていること。
    #[default]
    Substring,
    /// 打った文字が順番に現れること。間に何が挟まっていてもよい。
    Fuzzy,
}

impl MatchMode {
    pub fn toggled(self) -> Self {
        match self {
            MatchMode::Substring => MatchMode::Fuzzy,
            MatchMode::Fuzzy => MatchMode::Substring,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            MatchMode::Substring => "substr",
            MatchMode::Fuzzy => "fuzzy",
        }
    }
}

fn chars(hay: &str, ci: bool) -> Vec<(usize, char)> {
    hay.char_indices()
        .map(|(i, c)| {
            let c = if ci {
                c.to_lowercase().next().unwrap_or(c)
            } else {
                c
            };
            (i, c)
        })
        .collect()
}

fn span(h: &[(usize, char)], i: usize) -> (usize, usize) {
    let (off, c) = h[i];
    (off, off + c.len_utf8())
}

/// 出現位置をバイト範囲で返す。char 境界で走査するので日本語でも壊れない。
pub fn find_all(hay: &str, needle: &str, ci: bool) -> Vec<(usize, usize)> {
    if needle.is_empty() {
        return Vec::new();
    }
    let h = chars(hay, ci);
    let n = chars(needle, ci);
    let mut out = Vec::new();
    let mut i = 0;
    while i + n.len() <= h.len() {
        if (0..n.len()).all(|k| h[i + k].1 == n[k].1) {
            out.push((span(&h, i).0, span(&h, i + n.len() - 1).1));
            i += n.len();
        } else {
            i += 1;
        }
    }
    out
}

/// 部分列マッチ。打った文字が順に現れれば一致で、当たった 1 文字ずつを返す。
/// 位置を返すのは、どこに当たって一致と見なされたのかを画面で示すため。
pub fn find_subsequence(hay: &str, needle: &str, ci: bool) -> Vec<(usize, usize)> {
    if needle.is_empty() {
        return Vec::new();
    }
    let h = chars(hay, ci);
    let n = chars(needle, ci);
    let mut out = Vec::with_capacity(n.len());
    let mut i = 0;
    for (_, want) in &n {
        loop {
            if i >= h.len() {
                return Vec::new();
            }
            let hit = h[i].1 == *want;
            i += 1;
            if hit {
                out.push(span(&h, i - 1));
                break;
            }
        }
    }
    out
}

pub fn find(hay: &str, needle: &str, mode: MatchMode, ci: bool) -> Vec<(usize, usize)> {
    match mode {
        MatchMode::Substring => find_all(hay, needle, ci),
        MatchMode::Fuzzy => find_subsequence(hay, needle, ci),
    }
}

pub fn merged_ranges(
    text: &str,
    tokens: &[String],
    mode: MatchMode,
    ci: bool,
) -> Vec<(usize, usize)> {
    let mut all: Vec<(usize, usize)> = tokens
        .iter()
        .flat_map(|t| find(text, t, mode, ci))
        .collect();
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

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;

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

    /// fzf 的な部分列マッチ。substring では落ちるものが拾える。
    #[test]
    fn fuzzyは間に文字が挟まっても一致する() {
        assert_eq!(find_all("PhotoScrubberKit", "psk", true), vec![]);
        let hits = find_subsequence("PhotoScrubberKit", "psk", true);
        assert_eq!(hits.len(), 3, "当たった 1 文字ずつを返す");
        let text = "PhotoScrubberKit";
        let got: String = hits.iter().map(|&(a, b)| &text[a..b]).collect();
        assert_eq!(got, "PSK");
    }

    #[test]
    fn fuzzyでも順序が違えば一致しない() {
        assert!(find_subsequence("abc", "cb", true).is_empty());
        assert!(!find_subsequence("abc", "bc", true).is_empty());
    }

    #[test]
    fn fuzzyは日本語でもバイト境界が壊れない() {
        let text = "受注A案件";
        let hits = find_subsequence(text, "受件", true);
        assert_eq!(hits.len(), 2);
        let got: String = hits.iter().map(|&(a, b)| &text[a..b]).collect();
        assert_eq!(got, "受件");
    }

    #[test]
    fn モードで振る舞いが切り替わる() {
        assert!(find("abc", "ac", MatchMode::Substring, true).is_empty());
        assert!(!find("abc", "ac", MatchMode::Fuzzy, true).is_empty());
        assert_eq!(MatchMode::default(), MatchMode::Substring);
        assert_eq!(MatchMode::Substring.toggled(), MatchMode::Fuzzy);
        assert_eq!(MatchMode::Fuzzy.toggled(), MatchMode::Substring);
    }

    #[test]
    fn 日本語でもバイト境界が壊れない() {
        let hits = find_all("marutope/受注A", "受注", true);
        assert_eq!(hits.len(), 1);
        let (s, e) = hits[0];
        assert_eq!(&"marutope/受注A"[s..e], "受注");
    }
}
