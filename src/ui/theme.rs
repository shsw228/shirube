use ratatui::prelude::*;

// 端末の 16 色から取る。256 色の固定値は利用者のテーマとぶつかり、明るい背景では
// 暗いグレーが読めなくなる。Reset は端末の既定色。
pub const C_TEXT: Color = Color::Reset;

pub const C_MUTED: Color = Color::DarkGray;

pub const C_KEY: Color = Color::Blue;

pub const C_HIT: Color = Color::Yellow;

pub const C_SEL_BG: Color = Color::Blue;

pub const C_SEL_FG: Color = Color::White;

/// 前景と背景をまとめて塗り替える。個々の色を残すと背景に沈む文字が出る。
pub fn selected_style() -> Style {
    Style::new()
        .bg(C_SEL_BG)
        .fg(C_SEL_FG)
        .add_modifier(Modifier::BOLD)
}

/// 選択行では前景色を指定し直さないと、青地に青文字で沈む。
pub fn marker_style(selected: bool) -> Style {
    if selected {
        selected_style()
    } else {
        Style::new().fg(C_KEY).add_modifier(Modifier::BOLD)
    }
}

/// 色に加えて下線も引く。単色端末や選択行の上でも見つかるように。
pub fn hit_style(selected: bool) -> Style {
    let base = if selected {
        Style::new().bg(C_SEL_BG)
    } else {
        Style::new()
    };
    base.fg(C_HIT)
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
}

/// 一致範囲で色を分けた Span 列を作る。範囲は重ならない前提（find_all がそう返す）。
pub fn highlight(
    text: &str,
    ranges: &[(usize, usize)],
    base: Style,
    hit: Style,
) -> Vec<Span<'static>> {
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
