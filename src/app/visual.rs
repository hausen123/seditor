use super::App;
use super::Mode;
use crate::node::normalize;
use crate::node::Node;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;

/// Visualモードの選択の単位。
#[derive(Debug, PartialEq, Clone, Copy)]
pub(super) enum VisualKind {
    Char,
    Line,
}

impl App {
    /// vやVでVisualに入る。
    pub(super) fn enter_visual(&mut self, kind: VisualKind) {
        self.mode = Mode::Visual;
        self.visual_kind = kind;
        self.visual_anchor = self.cursor;
        self.visual_anchor_col = self.cursor_col;
    }
    /// 選択範囲を直前の選択として控える。modeは呼び出し
    /// 側が設定すること。
    ///
    /// rangeは呼び出し側が選択を崩す前
    /// （selfのcursorを動かす前）に求めて渡すこと。
    /// この関数自体はcursor/anchorを読み直さない。
    fn leave_visual(&mut self, range: (usize, usize)) {
        self.last_visual_range = Some(range);
    }
    /// 行単位で見た選択範囲（ノード番号、両端含む）。
    /// 描画側（view.rs）の反転表示にも使う。
    pub(super) fn visual_line_range(
        &self,
    ) -> (usize, usize) {
        let lo = self.visual_anchor.min(self.cursor);
        let hi = self.visual_anchor.max(self.cursor);
        (lo, hi)
    }
    /// 文字単位の選択範囲。アンカーとカーソルが別の
    /// ノードでもよく、vimの複数行にまたがる文字単位
    /// ビジュアルと同じく(開始ノード, 開始バイト位置,
    /// 終了ノード, 終了バイト位置)を返す。終了は選択に
    /// 含まれる文字の直後（半開区間）。
    /// 描画側（view.rs）の反転表示にも使う。
    pub(super) fn visual_char_span(
        &self,
    ) -> (usize, usize, usize, usize) {
        let anchor =
            (self.visual_anchor, self.visual_anchor_col);
        let cursor = (self.cursor, self.cursor_col);
        let (lo_node, lo_col, hi_node, hi_col) =
            if anchor <= cursor {
                (anchor.0, anchor.1, cursor.0, cursor.1)
            } else {
                (cursor.0, cursor.1, anchor.0, anchor.1)
            };
        let hi_text = &self.nodes[hi_node].text;
        let hi_col_end = match hi_text[hi_col..].chars().next()
        {
            Some(c) => hi_col + c.len_utf8(),
            None => hi_col,
        };
        (lo_node, lo_col, hi_node, hi_col_end)
    }
    /// 文字単位選択の中身を、削除せずノード列として
    /// 取り出す（normalize済み）。ノード全体が選択に
    /// 含まれていればそのまま、両端だけ一部なら
    /// そのノードのテキストを選択部分だけに削った
    /// コピーにする。yank(count)が
    /// normalize(&self.nodes[cursor..end])を使うのと
    /// 同じ形にして、pでの貼り方をノード単位のyy/dd
    /// と揃える。
    fn visual_char_register(&self) -> Vec<Node> {
        let (lo_node, lo_col, hi_node, hi_col_end) =
            self.visual_char_span();
        let mut nodes: Vec<Node> = self.nodes
            [lo_node..=hi_node]
            .to_vec();
        if lo_node == hi_node {
            nodes[0].text =
                nodes[0].text[lo_col..hi_col_end]
                    .to_string();
        } else {
            let last = nodes.len() - 1;
            nodes[last].text =
                nodes[last].text[..hi_col_end]
                    .to_string();
            nodes[0].text =
                nodes[0].text[lo_col..].to_string();
        }
        normalize(&nodes)
    }
    /// 文字単位選択を取り除く。ノード全体が選択範囲に
    /// 含まれていればddと同じくノードごと消し（子は
    /// 1段持ち上がる）、一部だけならそのノードの
    /// テキストの該当部分だけを削ってノードは残す
    /// （合体しない）。カーソルを置くべき
    /// (ノード, 桁)を返す。
    fn remove_char_span(&mut self) -> (usize, usize) {
        let (lo_node, lo_col, hi_node, hi_col_end) =
            self.visual_char_span();
        if lo_node == hi_node {
            self.nodes[lo_node]
                .text
                .drain(lo_col..hi_col_end);
            return (lo_node, lo_col);
        }
        let lo_full = lo_col == 0;
        let hi_full =
            hi_col_end == self.nodes[hi_node].text.len();

        if !hi_full {
            self.nodes[hi_node].text.drain(..hi_col_end);
        }
        if !lo_full {
            self.nodes[lo_node].text.drain(lo_col..);
        }

        let delete_start =
            if lo_full { lo_node } else { lo_node + 1 };
        let delete_end =
            if hi_full { hi_node + 1 } else { hi_node };
        if delete_start < delete_end {
            self.nodes.drain(delete_start..delete_end);
        }

        if self.nodes.is_empty() {
            self.nodes.push(Node {
                text: String::new(),
                depth: 0,
            });
        }

        self.repair();

        if lo_full {
            let node =
                delete_start.min(self.nodes.len() - 1);
            (node, 0)
        } else {
            (lo_node, lo_col)
        }
    }
    pub(super) fn handle_visual(&mut self, key: KeyEvent) {
        let KeyCode::Char(c) = key.code else {
            if key.code == KeyCode::Esc {
                let range = self.visual_line_range();
                self.leave_visual(range);
                self.mode = Mode::Normal;
            }
            return;
        };
        // 数字は回数として溜める。
        if self.pending.is_none()
            && c.is_ascii_digit()
            && (c != '0' || self.count.is_some())
        {
            let digit = c.to_digit(10).unwrap() as usize;
            self.count = Some(
                self.count.unwrap_or(0) * 10 + digit,
            );
            return;
        }
        // gg の1打鍵目。
        if self.pending.is_none() && c == 'g' {
            self.pending = Some(c);
            return;
        }
        let given = self.count.take();
        let count = given.unwrap_or(1);
        if let Some(first) = self.pending.take() {
            if (first, c) == ('g', 'g') {
                self.move_to(0);
            }
            return;
        }
        match c {
            'h' => {
                for _ in 0..count {
                    self.move_left();
                }
            }
            'l' => {
                for _ in 0..count {
                    self.move_right();
                }
            }
            // Char単位でノードを跨いでも、vim同様
            // アンカーからカーソルまでの複数ノードに
            // またがる選択として扱う（visual_char_span
            // 参照）。
            'j' => {
                for _ in 0..count {
                    self.move_down();
                }
            }
            'k' => {
                for _ in 0..count {
                    self.move_up();
                }
            }
            '0' => self.cursor_col = 0,
            '$' => self.cursor_col = self.text().len(),
            'G' => {
                let index = match given {
                    Some(number) => number - 1,
                    None => self.nodes.len() - 1,
                };
                self.move_to(index);
            }
            'v' => {
                if self.visual_kind == VisualKind::Char {
                    let range = self.visual_line_range();
                    self.leave_visual(range);
                    self.mode = Mode::Normal;
                } else {
                    self.visual_kind = VisualKind::Char;
                }
            }
            'V' => {
                if self.visual_kind == VisualKind::Line {
                    let range = self.visual_line_range();
                    self.leave_visual(range);
                    self.mode = Mode::Normal;
                } else {
                    self.visual_kind = VisualKind::Line;
                }
            }
            'y' => self.visual_yank(),
            'd' | 'x' => self.visual_delete(),
            'c' => self.visual_change(),
            '>' => self.visual_indent(true),
            '<' => self.visual_indent(false),
            ':' => {
                let range = self.visual_line_range();
                self.command = "'<,'>".to_string();
                self.message.clear();
                self.leave_visual(range);
                self.mode = Mode::Command;
            }
            _ => {}
        }
    }
    /// 現在の表示モードに応じたレジスタ。
    fn register_slot(&mut self) -> &mut Vec<Node> {
        if self.source_mode {
            &mut self.source_register
        } else {
            &mut self.register
        }
    }
    fn visual_yank(&mut self) {
        let range = self.visual_line_range();
        match self.visual_kind {
            VisualKind::Line => {
                let (lo, hi) = range;
                self.cursor = lo;
                self.yank(hi - lo + 1);
            }
            VisualKind::Char => {
                let cut = self.visual_char_register();
                let (lo_node, lo_col, _, _) =
                    self.visual_char_span();
                *self.register_slot() = cut;
                self.cursor = lo_node;
                self.cursor_col = lo_col;
            }
        }
        self.leave_visual(range);
        self.mode = Mode::Normal;
    }
    fn visual_delete(&mut self) {
        let range = self.visual_line_range();
        match self.visual_kind {
            VisualKind::Line => {
                let (lo, hi) = range;
                self.cursor = lo;
                self.begin_edit();
                self.delete_node(hi - lo + 1, true);
            }
            VisualKind::Char => {
                let cut = self.visual_char_register();
                self.begin_edit();
                let (node, col) = self.remove_char_span();
                *self.register_slot() = cut;
                self.cursor = node;
                self.cursor_col = col;
                self.record();
            }
        }
        self.leave_visual(range);
        self.mode = Mode::Normal;
    }
    fn visual_change(&mut self) {
        let range = self.visual_line_range();
        match self.visual_kind {
            VisualKind::Line => {
                let (lo, hi) = range;
                self.cursor = lo;
                self.begin_edit();
                self.delete_node(hi - lo + 1, true);
                self.open_above();
            }
            VisualKind::Char => {
                let cut = self.visual_char_register();
                self.begin_edit();
                let (node, col) = self.remove_char_span();
                *self.register_slot() = cut;
                self.cursor = node;
                self.cursor_col = col;
                self.record();
            }
        }
        self.leave_visual(range);
        self.mode = Mode::Insert;
    }
    /// V選択のみ。increaseがtrueなら>、falseなら<。
    ///
    /// 選択範囲の各部分木は、その根が選択に含まれる
    /// ときだけ1回上げ下げする。親を含む選択で子を
    /// 二重に上げ下げしないため、部分木の終端まで
    /// まとめて飛ばして処理済みを外す。
    fn visual_indent(&mut self, increase: bool) {
        let range = self.visual_line_range();
        if self.visual_kind != VisualKind::Line {
            self.leave_visual(range);
            self.mode = Mode::Normal;
            return;
        }
        let (lo, hi) = range;
        self.begin_edit();
        let mut i = lo;
        while i <= hi {
            self.cursor = i;
            let end = self.subtree_end(i);
            if increase {
                self.indent_subtree();
            } else {
                self.unindent_subtree();
            }
            i = end.max(i + 1);
        }
        self.leave_visual(range);
        self.mode = Mode::Normal;
    }
}
