use ratatui::prelude::Rect;

/// 画面の状態。スクロール位置とペインの矩形は描画のたびに決まるもので、
/// 選択やクエリと違って「アプリが何を指しているか」ではない。
/// ratatui の ListState と同じく、描画時に &mut で渡して更新する。
pub struct View {
    pub offset: usize,
    pub group_offset: usize,
    /// 直近に描いた一覧の行数。ページ移動と H/M/L が使う。
    pub height: usize,
    pub list: Rect,
    pub sidebar: Option<Rect>,
}

impl Default for View {
    /// height だけは 0 にしない。ページ移動や H/M/L が高さで割るため。
    fn default() -> Self {
        View {
            offset: 0,
            group_offset: 0,
            height: 1,
            list: Rect::default(),
            sidebar: None,
        }
    }
}

impl View {
    pub fn reset_scroll(&mut self) {
        self.offset = 0;
    }

    /// cursor が見えるところまで offset を寄せる。
    pub fn follow(&mut self, cursor: usize, height: usize) {
        if cursor < self.offset {
            self.offset = cursor;
        }
        if cursor >= self.offset + height {
            self.offset = cursor + 1 - height;
        }
    }

    pub fn contains(&self, rect: Rect, col: u16, row: u16) -> bool {
        col >= rect.x && col < rect.x + rect.width && row >= rect.y && row < rect.y + rect.height
    }
}
