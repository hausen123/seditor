mod editing;
mod io;
mod input;
mod search;
mod substitute;
mod ui;
mod view;
mod visual;

#[cfg(test)]
mod tests;

pub(crate) use ui::draw;

use crate::node::Node;

use std::path::PathBuf;

use ratatui::text::Text;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Wrap;

/// 巻き戻すために丸ごと控えておく状態。
#[derive(Clone)]
struct Snapshot {
    nodes: Vec<Node>,
    cursor: usize,
    cursor_col: usize,
    /// F2の切り替え自体は undo の1段として積まないが、
    /// これを一緒に控えておくことで、切り替えを挟んで
    /// 戻っても表示モードと内容の形が食い違わない。
    source_mode: bool,
}

#[derive(Debug, PartialEq)]
enum Mode {
    Normal,
    Insert,
    Command,
    Search,
    Confirm,
    Visual,
}

pub(crate) struct App {
    nodes: Vec<Node>,
    cursor: usize,
    cursor_col: usize,
    source_mode: bool,
    mode: Mode,
    /// dd や >> の1打鍵目。
    pending: Option<char>,
    /// :の入力中の文字列。:は含まない。
    command: String,
    /// / や ? の入力中の文字列。記号自体は含まない。
    search: String,
    /// 入力中・直前の検索が前方（/）か後方（?）か。
    search_forward: bool,
    /// n・N で繰り返すための直前の検索。
    /// パターンと方向（trueなら前方）を持つ。
    last_search: Option<(String, bool)>,
    /// `:&` `:&&` `g&` `~` のために覚えておく直前の
    /// `:s`。
    last_substitute: Option<substitute::LastSubstitute>,
    /// `c`フラグの対話確認の途中状態。
    confirm_state: Option<substitute::ConfirmState>,
    path: Option<PathBuf>,
    /// `:wt`で最後に指定した書き出し先。`.scm`用の
    /// pathとは別に持つ。
    tree_path: Option<PathBuf>,
    modified: bool,
    /// 画面下に出す一言。
    message: String,
    quit: bool,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// ヤンクした内容。深さは0からの相対値。
    register: Vec<Node>,
    /// ソース表示用の別レジスタ。
    ///
    /// 木の部分木をそのままソース表示に貼ると、
    /// 相対深さが混ざって表示が崩れるので分けてある。
    source_register: Vec<Node>,
    /// Visual中の選択の種類。Visual以外では意味を持たない。
    visual_kind: visual::VisualKind,
    /// Visualに入ったときのカーソルのノード番号。
    visual_anchor: usize,
    /// Visualに入ったときのcursor_col（文字単位のとき用）。
    visual_anchor_col: usize,
    /// 直前のVisual選択範囲（開始ノード番号, 終了ノード番号）。
    /// 両端含む。`'<,'>`のために抜けるたびに更新する。
    last_visual_range: Option<(usize, usize)>,
    /// 数字を溜めた回数指定。
    count: Option<usize>,
    /// 行番号を出すか。:set number で切り替える。
    number: bool,
    /// 画面最上部の行番号。
    scroll: usize,
    /// 木の表示の横スクロール位置（列数）。
    ///
    /// 行番号ガターは対象外で、全行共通の1本。
    /// 行ごとに別々にすると罫線がずれる。
    h_scroll: usize,
    /// 枠の内側の行数。描画時に入る。
    height: usize,
    /// 木の表示でガターを除いた横の文字数。
    /// 描画時に入る。
    width: usize,
    /// 各ノードが画面上で何行目から始まるか。
    ///
    /// scroll/height/カーソル追従は、ノード番号では
    /// なくこの「画面行」を基準にする。木の表示は
    /// 折り返さないので常に [0,1,2,...]（1ノード=1行）
    /// になる。ソース表示は折り返すので、ratatuiが
    /// 実際に使うのと同じアルゴリズムで1ノードが
    /// 何行になるか数える必要がある。そうしないと
    /// scrollの単位（画面行）とノード番号がずれ、
    /// カーソルが画面外に描画されることがある。
    /// 描画時に入る。
    row_offsets: Vec<usize>,
    /// 折り返した後の全体行数。描画時に入る。
    total_rows: usize,
    /// 編集の区切りで取った控え。
    ///
    /// 実際に変更が起きるまでundoには積まない。
    /// 何もしなかった i → Esc で空の段を作らないため。
    held: Option<Snapshot>,
}

/// 積んでおくアンドゥの段数。
const UNDO_LIMIT: usize = 100;

impl App {
    pub(crate) fn new() -> Self {
        Self {
            nodes: vec![
                Node {
                    text: String::new(),
                    depth: 0,
                },
            ],
            cursor: 0,
            cursor_col: 0,
            source_mode: false,
            mode: Mode::Normal,
            pending: None,
            command: String::new(),
            search: String::new(),
            search_forward: true,
            last_search: None,
            last_substitute: None,
            confirm_state: None,
            path: None,
            tree_path: None,
            modified: false,
            message: String::new(),
            quit: false,
            undo: Vec::new(),
            redo: Vec::new(),
            register: Vec::new(),
            source_register: Vec::new(),
            visual_kind: visual::VisualKind::Char,
            visual_anchor: 0,
            visual_anchor_col: 0,
            last_visual_range: None,
            count: None,
            number: false,
            scroll: 0,
            h_scroll: 0,
            height: 0,
            width: 0,
            row_offsets: Vec::new(),
            total_rows: 0,
            held: None,
        }
    }

    /// 起動時にファイルを開く。
    ///
    /// 存在すれば読み込み、存在しなければ新規ファイルの
    /// パスとして保持するだけにする。
    pub(crate) fn open(&mut self, path: PathBuf) {
        if path.exists() {
            self.load(&path);
        }
        self.path = Some(path);
    }

    /// row_offsets/total_rowsを最新にする。
    ///
    /// widthは実際にParagraphへ渡る幅（枠を除いた分）と
    /// 一致させること。ずれるとratatuiの折り返し計算と
    /// 食い違い、この仕組み自体が無意味になる。
    fn refresh_row_offsets(&mut self, width: u16) {
        let mut offsets =
            Vec::with_capacity(self.nodes.len());
        let mut total = 0;

        if self.source_mode && width > 0 {
            for line in self.tree_display() {
                offsets.push(total);
                // Paragraph::line_countは、実際の描画と
                // 同じWordWrapperで折り返し後の行数を
                // 数える公開APIなので、これを使えば
                // ratatuiの折り返しと必ず一致する。
                let height = Paragraph::new(Text::from(
                    vec![line],
                ))
                .wrap(Wrap { trim: false })
                .line_count(width)
                .max(1);

                total += height;
            }
        } else {
            // 木の表示は折り返さないので、常に
            // 1ノード=1行。
            for _ in 0..self.nodes.len() {
                offsets.push(total);
                total += 1;
            }
        }

        self.row_offsets = offsets;
        self.total_rows = total;
    }

    /// カーソルの画面上の行。row_offsetsが古い
    /// （draw()をまだ通していない）場合はcursorを
    /// そのまま使う。
    fn cursor_row(&self) -> usize {
        self.row_offsets
            .get(self.cursor)
            .copied()
            .unwrap_or(self.cursor)
    }

    /// row_offsetsが有効なときのノード数ぶんの
    /// 画面行数。無効なときはノード数をそのまま使う
    /// （height==0のときのfollow_cursor()と同じ
    /// 考え方）。
    fn total_rows(&self) -> usize {
        if self.row_offsets.len() == self.nodes.len() {
            self.total_rows
        } else {
            self.nodes.len()
        }
    }

    /// 画面行からノード番号を逆引きする。対象行以下で
    /// 一番近いノードを返す。
    ///
    /// row_offsetsがまだ描画を経ておらずnodesと数が
    /// 合わないときは、cursor_row()/total_rows()と
    /// 同じく「1ノード=1行」とみなして素通しする。
    fn node_at_row(&self, target: usize) -> usize {
        if self.row_offsets.len() != self.nodes.len() {
            return target;
        }

        let mut best = 0;
        let mut best_row = 0;

        for (index, &row) in
            self.row_offsets.iter().enumerate()
        {
            if row <= target && row >= best_row {
                best_row = row;
                best = index;
            }
        }

        best
    }

    /// カーソルが画面に入るよう最小限だけ動かす。
    ///
    /// 見えている間は動かさない。ノード番号ではなく
    /// 画面行（row_offsets）を基準にする。ソース表示は
    /// 折り返すので、1ノードが複数行になることがあり、
    /// ノード番号をそのままscrollに使うとずれる。
    fn follow_cursor(&mut self) {
        if self.height == 0 {
            return;
        }
        let row = self.cursor_row();
        if row < self.scroll {
            self.scroll = row;
        } else if row >= self.scroll + self.height {
            self.scroll = row - self.height + 1;
        }
    }

    /// カーソルが画面の中央に来るようscrollを合わせる。
    /// Mの逆（Mはカーソルをscrollに合わせて動かす）。
    ///
    /// 端では収まる範囲に収めるので、厳密に中央には
    /// ならないことがある（vimのzzと同じ）。
    fn center_on_cursor(&mut self) {
        if self.height == 0 {
            return;
        }
        let half = self.height / 2;
        let row = self.cursor_row();
        let target = row.saturating_sub(half);
        let last_scroll =
            self.total_rows().saturating_sub(self.height);
        self.scroll = target.min(last_scroll);
    }

    /// カーソルの画面上の桁を数える。
    ///
    /// プレフィクスと、カーソルより手前のテキストを
    /// 文字数で数える。桁の単位はここでは文字数とし、
    /// 全角文字の幅は数えない（他の桁計算と同じ簡略化）。
    fn cursor_column(&self, index: usize) -> usize {
        let prefix =
            self.tree_prefix(index).chars().count();

        let text = &self.nodes[index].text;

        let column = if text.is_empty() {
            0
        } else {
            self.cursor_col
        };

        prefix + text[..column].chars().count()
    }

    /// カーソルの桁が画面に入るよう、横方向も
    /// 最小限だけ動かす。follow_cursor()の横版。
    ///
    /// 見切れの印は左右で最大2列を食うので、その分を
    /// 差し引いた幅で判定する。そのまま width を使うと、
    /// 印を出したときにカーソルがちょうど印の裏に
    /// 隠れることがある。
    fn follow_cursor_horizontal(&mut self) {
        if self.width == 0 {
            return;
        }

        let visible = self.width.saturating_sub(2).max(1);

        let column = self.cursor_column(self.cursor);

        if column < self.h_scroll {
            self.h_scroll = column;
        } else if column >= self.h_scroll + visible {
            self.h_scroll = column - visible + 1;
        }
    }

    /// 画面をlines行送る。
    ///
    /// カーソルを動かすだけではfollow_cursorが
    /// 最小限しか送らないので、scrollも直接動かす。
    /// 両方を同じだけ動かすと画面上の行が変わらない。
    fn scroll_page(&mut self, lines: isize) {
        let last_scroll = self
            .total_rows()
            .saturating_sub(self.height);

        self.scroll = self
            .scroll
            .saturating_add_signed(lines)
            .min(last_scroll);

        let last = self.nodes.len() - 1;

        let target_row = self
            .cursor_row()
            .saturating_add_signed(lines);

        self.cursor =
            self.node_at_row(target_row).min(last);

        // scrollが端で止まったときは画面内に戻す。
        // 行（row_offsets）で範囲を決め、ノードに
        // 逆引きしてからクランプする。
        let bottom_row = (self.scroll + self.height)
            .saturating_sub(1);

        let min_node = self.node_at_row(self.scroll);
        let max_node =
            self.node_at_row(bottom_row).min(last);

        self.cursor =
            self.cursor.clamp(min_node, max_node);

        self.clamp_cursor_col();
    }

    /// カーソルまたは画面をlines行動かす。
    fn scroll_by(&mut self, lines: isize) {
        let last = self.nodes.len() - 1;
        self.cursor = self
            .cursor
            .saturating_add_signed(lines)
            .min(last);
        self.clamp_cursor_col();
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            nodes: self.nodes.clone(),
            cursor: self.cursor,
            cursor_col: self.cursor_col,
            source_mode: self.source_mode,
        }
    }

    /// 編集の区切り。
    ///
    /// 挿入モードに入るときと、Normalモードの
    /// 編集命令の直前に呼ぶ。挿入中は呼ばないので、
    /// i から Esc までが1段にまとまる。
    fn begin_edit(&mut self) {
        self.held = Some(self.snapshot());
    }

    /// 変更が起きたときに呼ぶ。
    ///
    /// 控えがあれば1回だけ確定する。2文字目以降は
    /// 控えが空なので何もしない。
    fn record(&mut self) {
        self.modified = true;

        let Some(held) = self.held.take() else {
            return;
        };

        self.undo.push(held);

        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }

        self.redo.clear();
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.nodes = snapshot.nodes;
        self.cursor = snapshot.cursor;
        self.cursor_col = snapshot.cursor_col;
        self.source_mode = snapshot.source_mode;
        self.held = None;
        self.modified = true;
    }

    fn undo(&mut self) {
        let Some(snapshot) = self.undo.pop() else {
            self.message =
                "already at oldest change".to_string();
            return;
        };

        self.redo.push(self.snapshot());
        self.restore(snapshot);
    }

    fn redo(&mut self) {
        let Some(snapshot) = self.redo.pop() else {
            self.message =
                "already at newest change".to_string();
            return;
        };

        self.undo.push(self.snapshot());
        self.restore(snapshot);
    }

    /// カーソルのあるノードのテキスト。
    fn text(&self) -> &str {
        &self.nodes[self.cursor].text
    }

    /// cursor_colを現在のノードのテキストに合わせて
    /// クランプする。
    ///
    /// text().len()に対して素朴にminを取るだけだと、
    /// 移動元のノードでは境界だった位置が移動先の
    /// マルチバイト文字の途中に来ることがある
    /// （例: "審査基準"の7バイト目は'基'の途中）。
    /// is_char_boundary()で境界まで戻す。
    pub(super) fn clamp_cursor_col(&mut self) {
        let text = self.text();
        let mut col = self.cursor_col.min(text.len());
        while !text.is_char_boundary(col) {
            col -= 1;
        }
        self.cursor_col = col;
    }

    fn move_to(&mut self, index: usize) {
        self.cursor = index.min(self.nodes.len() - 1);
        self.clamp_cursor_col();
    }
}
