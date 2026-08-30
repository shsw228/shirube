/// 一覧に並ぶ 1 件。path が正、owner/repo は表示のために切り出した控え。
pub struct Entry {
    pub path: String,
    pub group: usize,
    pub owner: String,
    pub repo: String,
}

/// パス末尾 2 階層を owner/repo として切り出す。ghq の表示に合わせている。
pub fn split_tail(path: &str) -> (String, String) {
    let parts: Vec<&str> = path.trim_end_matches('/').split('/').collect();
    let repo = parts.last().copied().unwrap_or(path).to_string();
    let owner = if parts.len() >= 2 {
        parts[parts.len() - 2].to_string()
    } else {
        repo.clone()
    };
    (owner, repo)
}
