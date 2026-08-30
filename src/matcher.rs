/// 出現位置をバイト範囲で返す。char 境界で走査するので日本語でも壊れない。
pub fn find_all(hay: &str, needle: &str, ci: bool) -> Vec<(usize, usize)> {
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

pub fn merged_ranges(text: &str, tokens: &[String], ci: bool) -> Vec<(usize, usize)> {
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

    #[test]
    fn 日本語でもバイト境界が壊れない() {
        let hits = find_all("marutope/受注A", "受注", true);
        assert_eq!(hits.len(), 1);
        let (s, e) = hits[0];
        assert_eq!(&"marutope/受注A"[s..e], "受注");
    }
}
