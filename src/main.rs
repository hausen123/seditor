mod reader;

use reader::Datum;

use std::fs;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use crossterm::{
    event::{
        self,
        DisableBracketedPaste,
        EnableBracketedPaste,
        Event,
        KeyCode,
        KeyEvent,
        KeyEventKind,
        KeyModifiers,
    },
    execute,
    terminal::{
        disable_raw_mode,
        enable_raw_mode,
        EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};

use ratatui::{
    prelude::*,
    widgets::{
        Block,
        Borders,
        Paragraph,
        Wrap,
    },
};

const WIDTH: usize = 80;

/// 子を並べる位置の決め方。
enum Indent {
    /// 先頭のn個を見出し行に残し、残りを+2桁に置く。
    Body(usize),
    /// 子を第1子の桁に揃える。
    Align,
}

/// 幅に収まっても必ず改行する形。
fn always_break(text: &str) -> bool {
    matches!(
        text,
        "define" | "lambda" | "let" | "let*" | "letrec"
            | "letrec*" | "when" | "unless" | "case"
            | "do" | "begin" | "cond"
    )
}

/// outputの末尾がいま何桁目にあるか。
fn current_column(output: &str) -> usize {
    match output.rfind('\n') {
        Some(position) => {
            output[position + 1..].chars().count()
        }
        None => output.chars().count(),
    }
}

#[derive(Debug, Clone)]
struct Node {
    text: String,
    depth: usize,
}

/// 読み取ったデータをノード列に直す。
fn nodes_from(data: &[Datum]) -> Vec<Node> {
    let mut nodes = Vec::new();

    for datum in data {
        emit(datum, 0, &mut nodes);
    }

    if nodes.is_empty() {
        nodes.push(Node {
            text: String::new(),
            depth: 0,
        });
    }

    nodes
}

/// マーカーの連なりがアトム1つに行き着くなら、
/// 印の文字を連結したテキストを返す。
///
/// '`x のように印が複数重なっていても、最終的に
/// アトムなら1ノードに畳めるため。リストや
/// コメントに行き着く場合はNone。
fn atom_chain(datum: &Datum) -> Option<String> {
    match datum {
        Datum::Atom(text) => Some(text.clone()),
        Datum::Marker(mark, child) => {
            atom_chain(child)
                .map(|rest| format!("{}{}", mark, rest))
        }
        _ => None,
    }
}

fn emit(
    datum: &Datum,
    depth: usize,
    nodes: &mut Vec<Node>,
) {
    match datum {
        Datum::Atom(text) | Datum::Comment(text) => {
            nodes.push(Node {
                text: text.clone(),
                depth,
            })
        }
        Datum::Marker(mark, child) => {
            emit_marker(mark, child, depth, nodes);
        }
        Datum::List(items) => {
            // 先頭がアトムなら見出しに置く。
            //
            // ただし1要素のリストは見出しにすると
            // 子を持たないノードになり、(f)ではなく
            // fとして出てしまう。
            let head = match items.first() {
                Some(Datum::Atom(text))
                    if items.len() >= 2 =>
                {
                    Some(text.clone())
                }
                _ => None,
            };
            let rest = match &head {
                Some(text) => {
                    nodes.push(Node {
                        text: text.clone(),
                        depth,
                    });
                    &items[1..]
                }
                None => {
                    nodes.push(Node {
                        text: String::new(),
                        depth,
                    });
                    &items[..]
                }
            };
            for item in rest {
                emit(item, depth + 1, nodes);
            }
        }
    }
}

/// マーカーを書き出す。emit()とemit_quoted()の
/// どちらから呼ばれても同じ規則になる。
///
/// アトムに行き着くなら1ノードに畳む。リストなら、
/// 空リストだけ例外的に◦を挟み、それ以外は◦を
/// 挟まずマーカー自身が要素を直接子に持つ（クォートの
/// 中は呼び出しではなくただのデータの並びなので、
/// 先頭を見出しにする理由が無い）。マーカーが
/// 続く場合はそのまま連結する。
fn emit_marker(
    mark: &str,
    child: &Datum,
    depth: usize,
    nodes: &mut Vec<Node>,
) {
    if let Some(rest) = atom_chain(child) {
        nodes.push(Node {
            text: format!("{}{}", mark, rest),
            depth,
        });
        return;
    }

    nodes.push(Node {
        text: mark.to_string(),
        depth,
    });

    match child {
        Datum::List(items) if items.is_empty() => {
            // '() だけ例外的に◦を挟む。
            nodes.push(Node {
                text: String::new(),
                depth: depth + 1,
            });
        }
        Datum::List(items) => {
            for item in items {
                emit_quoted(item, depth + 1, nodes);
            }
        }
        _ => emit(child, depth + 1, nodes),
    }
}

/// クォートの中を書き出す。emit()と違い、リストは
/// 先頭がアトムでも見出しにせず、常に◦にする
/// （マーカーの付いていない入れ子のリストには
/// attach先が無いため）。
fn emit_quoted(
    datum: &Datum,
    depth: usize,
    nodes: &mut Vec<Node>,
) {
    match datum {
        Datum::Atom(text) | Datum::Comment(text) => {
            nodes.push(Node {
                text: text.clone(),
                depth,
            })
        }
        Datum::Marker(mark, child) => {
            emit_marker(mark, child, depth, nodes);
        }
        Datum::List(items) => {
            nodes.push(Node {
                text: String::new(),
                depth,
            });
            for item in items {
                emit_quoted(item, depth + 1, nodes);
            }
        }
    }
}

/// 積んでおくアンドゥの段数。
const UNDO_LIMIT: usize = 100;

/// 深さをブロック内の最小に合わせて0からにする。
///
/// 先頭を基準にすると、後ろに浅いノードが続く場合に
/// 負になってしまう。
fn normalize(nodes: &[Node]) -> Vec<Node> {
    let base = nodes
        .iter()
        .map(|node| node.depth)
        .min()
        .unwrap_or(0);

    nodes
        .iter()
        .map(|node| Node {
            text: node.text.clone(),
            depth: node.depth - base,
        })
        .collect()
}

/// 行番号の最小の桁数。
///
/// ノード数が9→10と増えるたびに桁が変わると
/// 木全体が横にずれるので、下限を決めておく。
const NUMBER_WIDTH: usize = 3;

/// count行に行番号を振るときの桁幅。空白1つを含む。
fn number_width(count: usize) -> usize {
    let digits = count.to_string().len();
    digits.max(NUMBER_WIDTH) + 1
}

/// 淡く表示する行番号。
fn number_span(
    line: usize,
    width: usize,
) -> Span<'static> {
    Span::styled(
        format!("{:>1$} ", line, width - 1),
        Style::default().add_modifier(Modifier::DIM),
    )
}

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
}

struct App {
    nodes: Vec<Node>,
    cursor: usize,
    cursor_col: usize,
    source_mode: bool,
    mode: Mode,
    /// dd や >> の1打鍵目。
    pending: Option<char>,
    /// :の入力中の文字列。:は含まない。
    command: String,
    path: Option<PathBuf>,
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
    /// 編集の区切りで取った控え。
    ///
    /// 実際に変更が起きるまでundoには積まない。
    /// 何もしなかった i → Esc で空の段を作らないため。
    held: Option<Snapshot>,
}

impl App {
    fn new() -> Self {
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
            path: None,
            modified: false,
            message: String::new(),
            quit: false,
            undo: Vec::new(),
            redo: Vec::new(),
            register: Vec::new(),
            source_register: Vec::new(),
            count: None,
            number: false,
            scroll: 0,
            h_scroll: 0,
            height: 0,
            width: 0,
            held: None,
        }
    }

    // ------------------------------------------------------------
    // スクロール
    // ------------------------------------------------------------

    /// カーソルが画面に入るよう最小限だけ動かす。
    ///
    /// 見えている間は動かさない。
    fn follow_cursor(&mut self) {
        if self.height == 0 {
            return;
        }
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + self.height
        {
            self.scroll =
                self.cursor - self.height + 1;
        }
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
            .nodes
            .len()
            .saturating_sub(self.height);

        self.scroll = self
            .scroll
            .saturating_add_signed(lines)
            .min(last_scroll);

        let last = self.nodes.len() - 1;

        self.cursor = self
            .cursor
            .saturating_add_signed(lines)
            .min(last);

        // scrollが端で止まったときは画面内に戻す。
        let bottom = (self.scroll + self.height)
            .saturating_sub(1)
            .min(last);

        self.cursor =
            self.cursor.clamp(self.scroll, bottom);

        self.cursor_col =
            self.cursor_col.min(self.text().len());
    }

    /// カーソルまたは画面をlines行動かす。
    fn scroll_by(&mut self, lines: isize) {
        let last = self.nodes.len() - 1;
        self.cursor = self
            .cursor
            .saturating_add_signed(lines)
            .min(last);
        self.cursor_col =
            self.cursor_col.min(self.text().len());
    }

    // ------------------------------------------------------------
    // アンドゥ
    // ------------------------------------------------------------

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
                "これ以上戻れません".to_string();
            return;
        };

        self.redo.push(self.snapshot());
        self.restore(snapshot);
    }

    fn redo(&mut self) {
        let Some(snapshot) = self.redo.pop() else {
            self.message =
                "これ以上やり直せません".to_string();
            return;
        };

        self.undo.push(self.snapshot());
        self.restore(snapshot);
    }

    // ------------------------------------------------------------
    // コマンド
    // ------------------------------------------------------------

    /// :で始まる1行を実行する。
    fn run_command(&mut self, line: &str) {
        let mut words = line.split_whitespace();

        let Some(name) = words.next() else {
            return;
        };

        // :42 で42行目へ、:$ で末尾へ。
        let line = match name {
            "$" => Some(usize::MAX),
            _ => name.parse::<usize>().ok(),
        };

        if let Some(line) = line {
            self.move_to(line.saturating_sub(1));
            return;
        }

        if name == "set" {
            // :set number nonumber のように並べられる。
            let options: Vec<&str> = words.collect();
            if options.is_empty() {
                self.message =
                    "設定項目がありません".to_string();
                return;
            }
            for option in options {
                self.set_option(option);
            }
            return;
        }

        let argument = words.next();

        match name {
            "w" => {
                self.write(argument);
            }
            "wq" | "x" => {
                if self.write(argument) {
                    self.quit = true;
                }
            }
            "q" => {
                if self.modified {
                    self.message =
                        "変更が保存されていません。\
                         捨てるなら :q! です"
                            .to_string();
                } else {
                    self.quit = true;
                }
            }
            "q!" => self.quit = true,
            "e" => {
                self.edit(argument);
            }
            "e!" => {
                self.modified = false;
                self.edit(argument);
            }
            _ => {
                self.message =
                    format!("不明なコマンドです: {}", name)
            }
        }
    }

    /// :set の項目を1つ処理する。
    fn set_option(&mut self, option: &str) {
        match option {
            "number" | "nu" => self.number = true,
            "nonumber" | "nonu" => self.number = false,
            "number!" | "nu!" => {
                self.number = !self.number
            }
            _ => {
                self.message = format!(
                    "不明な設定項目です: {}",
                    option
                )
            }
        }
    }

    // ------------------------------------------------------------
    // ソース表示（F2）
    // ------------------------------------------------------------

    /// F2。木とソース表示を切り替える。
    fn toggle_source_view(&mut self) {
        if self.source_mode {
            self.source_to_tree();
        } else {
            self.tree_to_source();
        }
    }

    /// 木をソース表示に変換する。
    ///
    /// depth:0のノード列として持たせ、既存の編集
    /// エンジン（insert_char、undo、カーソル反転など）
    /// をそのまま使い回す。カーソルは、印字したときに
    /// 元のノードが始まった行・桁に合わせる。
    fn tree_to_source(&mut self) {
        let (source, positions) =
            self.to_scheme_with_positions();

        let (line, start) =
            Self::position_of(self.cursor, &positions);

        // position_ofが返すのはノードの印字が始まる桁。
        // ノードの中でのカーソルの位置も足す。畳んだ親に
        // 落ちたとき（cursorが自分のノードでないとき）は
        // 意味が無いので足さない。
        //
        // 子を持つ（または◦の）ノードは開き括弧が
        // 先頭に付くので、テキスト自体はその1文字後ろ
        // から始まる。
        let column = if positions[self.cursor].is_some() {
            let text = &self.nodes[self.cursor].text;
            let has_paren = !self.is_marker(self.cursor)
                && (!self.children(self.cursor).is_empty()
                    || text.trim().is_empty());
            let paren = if has_paren
                && !text.trim().is_empty()
            {
                1
            } else {
                0
            };
            start
                + paren
                + text[..self.cursor_col]
                    .chars()
                    .count()
        } else {
            start
        };

        self.nodes = source
            .lines()
            .map(|line| Node {
                text: line.to_string(),
                depth: 0,
            })
            .collect();

        if self.nodes.is_empty() {
            self.nodes.push(Node {
                text: String::new(),
                depth: 0,
            });
        }

        self.cursor =
            line.min(self.nodes.len() - 1);

        // columnは文字数。cursor_colはバイト位置で
        // 扱っているので変換する。
        let text = &self.nodes[self.cursor].text;

        self.cursor_col = text
            .char_indices()
            .nth(column)
            .map(|(i, _)| i)
            .unwrap_or(text.len());

        self.source_mode = true;
    }

    /// ソース表示の各行を連結し、読み直して木に戻す。
    ///
    /// 壊れた括弧のまま木に戻すと表示が崩れるので、
    /// パースに失敗したらソース表示に留まる。カーソルは
    /// 今いた行以下で一番近いノードに合わせる。桁までは
    /// 対応しない（1行に複数ノードが同居しうるため）。
    fn source_to_tree(&mut self) {
        let text: String = self
            .nodes
            .iter()
            .map(|node| node.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        let line = self.cursor;

        let reading = match reader::read(&text) {
            Ok(reading) => reading,
            Err(error) => {
                self.message = format!(
                    "木に戻せません: {}",
                    error
                );
                return;
            }
        };

        self.nodes = nodes_from(&reading.data);

        let (_, positions) =
            self.to_scheme_with_positions();

        self.cursor = Self::node_at_line(&positions, line);
        self.cursor_col = 0;
        self.source_mode = false;
    }

    /// ファイルを読む。読めたらtrueを返す。
    fn edit(&mut self, argument: Option<&str>) -> bool {
        if self.modified {
            self.message =
                "変更が保存されていません。\
                 捨てるなら :e! です"
                    .to_string();
            return false;
        }

        if let Some(name) = argument {
            self.path = Some(PathBuf::from(name));
        }

        let Some(path) = self.path.clone() else {
            self.message =
                "ファイル名がありません".to_string();
            return false;
        };

        self.load(&path)
    }

    fn load(&mut self, path: &Path) -> bool {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) => {
                self.message =
                    format!("読めません: {}", error);
                return false;
            }
        };

        let reading = match reader::read(&text) {
            Ok(reading) => reading,
            Err(error) => {
                self.message = format!(
                    "{} を読めません: {}",
                    path.display(),
                    error
                );
                return false;
            }
        };

        self.nodes = nodes_from(&reading.data);
        self.cursor = 0;
        self.cursor_col = 0;
        self.modified = false;
        // ソース表示中に別ファイルを開くと食い違うので、
        // 木の表示に戻す。
        self.source_mode = false;
        // ファイルが入れ替わるので履歴は捨てる。
        self.undo.clear();
        self.redo.clear();
        self.held = None;

        self.message = if reading.hoisted > 0 {
            format!(
                "{} を読みました。\
                 式の中にあったコメント{}件を\
                 行頭に出しました",
                path.display(),
                reading.hoisted
            )
        } else {
            format!("{} を読みました", path.display())
        };

        true
    }

    /// ファイルに書く。書けたらtrueを返す。
    fn write(&mut self, argument: Option<&str>) -> bool {
        if let Some(name) = argument {
            self.path = Some(PathBuf::from(name));
        }

        let Some(path) = self.path.clone() else {
            self.message =
                "ファイル名がありません".to_string();
            return false;
        };

        // ソース表示中はdepth:0のフラットな行なので、
        // to_scheme()に通さずそのまま連結する。
        let mut text = if self.source_mode {
            self.nodes
                .iter()
                .map(|node| node.text.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            self.to_scheme()
        };
        text.push('\n');

        match fs::write(&path, text) {
            Ok(()) => {
                self.modified = false;
                self.message = format!(
                    "{} に書き込みました",
                    path.display()
                );
                true
            }
            Err(error) => {
                self.message =
                    format!("書き込めません: {}", error);
                false
            }
        }
    }

    /// カーソルのあるノードのテキスト。
    fn text(&self) -> &str {
        &self.nodes[self.cursor].text
    }

    fn move_to(&mut self, index: usize) {
        self.cursor = index.min(self.nodes.len() - 1);
        self.cursor_col =
            self.cursor_col.min(self.text().len());
    }

    /// カーソルの1つ上に同じ深さの空ノードを作る。
    fn open_above(&mut self) {
        let depth = self.nodes[self.cursor].depth;
        self.nodes.insert(
            self.cursor,
            Node {
                text: String::new(),
                depth,
            },
        );
        self.cursor_col = 0;
        self.record();
    }

    /// カーソル位置の1文字を消す。
    ///
    /// ◦ には消す文字が無いので、ddと同じく
    /// ノードごと消して子を1段持ち上げる。
    /// yankはNormalモードのxのときだけtrue。
    fn delete_char(&mut self, yank: bool) {
        if self.text().is_empty() {
            self.delete_node(1, yank);
            return;
        }

        let column = self.cursor_col;
        let node = &mut self.nodes[self.cursor];

        if column >= node.text.len() {
            // 末尾ならBackspaceの逆で、次のノードの
            // テキストを引き込んで合流する。
            if self.cursor + 1 >= self.nodes.len() {
                return;
            }

            let text =
                self.nodes[self.cursor + 1].text.clone();

            self.nodes[self.cursor].text.push_str(&text);
            self.nodes.remove(self.cursor + 1);

            // 引き込んだノードに子があれば深さが
            // 飛ぶので、dd と同じく1段持ち上げる。
            self.repair();

            self.record();
            return;
        }

        if let Some(c) = node.text[column..].chars().next()
        {
            node.text
                .drain(column..column + c.len_utf8());
            self.record();
        }
    }

    fn insert_char(&mut self, c: char) {
        let node = &mut self.nodes[self.cursor];

        node.text.insert(self.cursor_col, c);
        self.cursor_col += c.len_utf8();
        self.record();
    }

    /// 貼り付けたテキストをカーソル位置に反映する。
    /// 改行のたびにEnterと同じくノードを分ける。
    fn paste_text(&mut self, text: &str) {
        // \r\n の \r は無視する。
        let text = text.replace('\r', "");
        let mut lines = text.split('\n');

        if let Some(first) = lines.next() {
            for c in first.chars() {
                self.insert_char(c);
            }
        }

        for line in lines {
            self.enter();
            for c in line.chars() {
                self.insert_char(c);
            }
        }
    }

    /// bracketed pasteで届いたテキストを反映する。
    ///
    /// コマンド行は1行しか持てないので最初の行だけ。
    /// Normalモードでの貼り付けは、そのままだと
    /// 文字列がキー入力として解釈されコマンド列に
    /// なってしまう危険があるため、1つの編集として
    /// 直接テキストを挿入する（vimと同じ扱い）。
    fn handle_paste(&mut self, text: &str) {
        match self.mode {
            Mode::Command => {
                if let Some(first) = text.lines().next() {
                    self.command.push_str(first);
                }
            }
            Mode::Insert => {
                self.paste_text(text);
            }
            Mode::Normal => {
                self.begin_edit();
                self.paste_text(text);
            }
        }
    }

    /// カーソルの手前の1文字を消す。
    ///
    /// ◦ の上ではノードごと消して、前のノードの
    /// 末尾に戻る。Enterを取り消す動きになる。
    /// レジスタは汚さない。
    fn backspace(&mut self) {
        if self.text().is_empty() {
            if self.cursor == 0 {
                self.delete_node(1, false);
                self.cursor_col = 0;
                return;
            }

            // 消すと末尾のときにdelete_nodeが
            // カーソルを前に丸めるので、消す前の
            // 位置から前のノードを決めておく。
            let previous = self.cursor - 1;

            self.delete_node(1, false);

            self.cursor = previous;
            self.cursor_col = self.text().len();

            return;
        }

        if self.cursor_col == 0 {
            // 先頭ならEnterの逆で、前のノードの
            // 末尾にテキストをくっつけて合流する。
            if self.cursor == 0 {
                return;
            }

            let text = self.nodes[self.cursor]
                .text
                .clone();

            let join_at =
                self.nodes[self.cursor - 1].text.len();

            self.nodes[self.cursor - 1]
                .text
                .push_str(&text);

            self.nodes.remove(self.cursor);

            self.cursor -= 1;
            self.cursor_col = join_at;

            // 消したノードに子があれば深さが飛ぶので、
            // dd と同じく1段持ち上げる。
            self.repair();

            self.record();
            return;
        }

        let node = &mut self.nodes[self.cursor];

        if let Some(c) = node.text[..self.cursor_col]
            .chars()
            .next_back()
        {
            let len = c.len_utf8();

            node.text.drain(
                self.cursor_col - len..self.cursor_col
            );

            self.cursor_col -= len;
            self.record();
        }
    }

    /// カーソルから後ろのテキストを次のノードに移す。
    ///
    /// 文の途中でEnterしたとき、その場で分かれる
    /// ようにする。カーソルが末尾なら次は空になる。
    fn enter(&mut self) {
        let depth = self.nodes[self.cursor].depth;
        let column = self.cursor_col;

        let rest = self.nodes[self.cursor]
            .text
            .split_off(column);

        self.nodes.insert(
            self.cursor + 1,
            Node { text: rest, depth },
        );

        self.cursor += 1;
        self.cursor_col = 0;
        self.record();
    }

    /// ソース表示は全行depth:0のフラットな行なので、
    /// 深さを変える操作は意味を持たない上に、上限計算
    /// （直前ノード+1）が常に1になり簡単に罫線が
    /// 出てきてしまう。無効にする。
    fn indent(&mut self) {
        if self.source_mode || self.cursor == 0 {
            return;
        }

        let previous_depth =
            self.nodes[self.cursor - 1].depth;

        let current_depth =
            self.nodes[self.cursor].depth;

        if current_depth < previous_depth + 1 {
            self.nodes[self.cursor].depth += 1;
            self.record();
        }
    }

    fn unindent(&mut self) {
        if !self.source_mode
            && self.nodes[self.cursor].depth > 0
        {
            self.nodes[self.cursor].depth -= 1;
            self.record();
        }
    }

    /// カーソルの部分木を子孫ごと1段下げる。
    ///
    /// indent()をカーソルのノードだけに使うと、
    /// 子孫の深さは変わらないまま親子関係が崩れる。
    /// 上限はindent()と同じ。
    fn indent_subtree(&mut self) {
        if self.source_mode || self.cursor == 0 {
            return;
        }

        let previous_depth =
            self.nodes[self.cursor - 1].depth;

        let current_depth =
            self.nodes[self.cursor].depth;

        if current_depth >= previous_depth + 1 {
            return;
        }

        let end = self.subtree_end(self.cursor);

        for node in &mut self.nodes[self.cursor..end] {
            node.depth += 1;
        }

        self.record();
    }

    /// カーソルの部分木を子孫ごと1段上げる。
    fn unindent_subtree(&mut self) {
        if self.source_mode
            || self.nodes[self.cursor].depth == 0
        {
            return;
        }

        let end = self.subtree_end(self.cursor);

        for node in &mut self.nodes[self.cursor..end] {
            node.depth -= 1;
        }

        self.record();
    }

    fn move_up(&mut self) {
        if self.cursor == 0 {
            return;
        }

        self.cursor -= 1;

        self.cursor_col = self.cursor_col.min(
            self.nodes[self.cursor].text.len()
        );
    }

    fn move_down(&mut self) {
        if self.cursor + 1 >= self.nodes.len() {
            return;
        }

        self.cursor += 1;

        self.cursor_col = self.cursor_col.min(
            self.nodes[self.cursor].text.len()
        );
    }

    fn move_left(&mut self) {
        if self.cursor_col == 0 {
            return;
        }

        if let Some(c) = self.nodes[self.cursor]
            .text[..self.cursor_col]
            .chars()
            .next_back()
        {
            self.cursor_col -= c.len_utf8();
        }
    }

    fn move_right(&mut self) {
        let len = self.nodes[self.cursor].text.len();

        if self.cursor_col >= len {
            return;
        }

        if let Some(c) = self.nodes[self.cursor]
            .text[self.cursor_col..]
            .chars()
            .next()
        {
            self.cursor_col += c.len_utf8();
        }
    }

    /// 深さの不変条件を復元する。
    ///
    /// 各ノードの深さは1つ前のノード + 1 を超えない。
    /// 超えたノードはchildren()に拾われず、
    /// 出力から黙って消えてしまう。
    ///
    /// 1ノードの削除に当てると「子を1段持ち上げる」に
    /// なり、複数まとめて消したときの穴も同じ規則で
    /// 塞がる。
    fn repair(&mut self) {
        for i in 0..self.nodes.len() {
            let limit = if i == 0 {
                0
            } else {
                self.nodes[i - 1].depth + 1
            };
            if self.nodes[i].depth > limit {
                self.nodes[i].depth = limit;
            }
        }
    }

    /// カーソルから count 個のノードを消す。
    ///
    /// yankがtrueなら消したぶんをレジスタに入れる。
    /// Backspace/Deleteはレジスタを汚さないよう
    /// falseで呼ぶ。子は深さの復元によって持ち上がる。
    fn delete_node(&mut self, count: usize, yank: bool) {
        let end = (self.cursor + count)
            .min(self.nodes.len());

        if yank {
            let cut =
                normalize(&self.nodes[self.cursor..end]);

            if self.source_mode {
                self.source_register = cut;
            } else {
                self.register = cut;
            }
        }

        self.nodes.drain(self.cursor..end);

        if self.nodes.is_empty() {
            self.nodes.push(Node {
                text: String::new(),
                depth: 0,
            });
        }

        if self.cursor >= self.nodes.len() {
            self.cursor = self.nodes.len() - 1;
        }

        self.repair();

        self.cursor_col =
            self.cursor_col.min(self.text().len());
        self.record();
    }

    /// indexの部分木の直後の位置。
    ///
    /// 深さがindexと同じか浅くなる手前まで。
    fn subtree_end(&self, index: usize) -> usize {
        let depth = self.nodes[index].depth;
        let mut i = index + 1;

        while i < self.nodes.len()
            && self.nodes[i].depth > depth
        {
            i += 1;
        }

        i
    }

    /// カーソルの部分木のノード数。
    ///
    /// Y と D はこれを個数として渡すので、
    /// yank と delete_node をそのまま使える。
    fn subtree_len(&self) -> usize {
        self.subtree_end(self.cursor) - self.cursor
    }

    /// カーソルから count 個のノードをヤンクする。
    fn yank(&mut self, count: usize) {
        let end = (self.cursor + count)
            .min(self.nodes.len());

        let cut =
            normalize(&self.nodes[self.cursor..end]);

        let length = cut.len();

        if self.source_mode {
            self.source_register = cut;
        } else {
            self.register = cut;
        }

        self.message =
            format!("{}ノードをヤンクしました", length);
    }

    /// レジスタの内容を貼る。木とソース表示で
    /// 別々のレジスタを使う。部分木の相対深さが
    /// 平坦なソース表示に混ざって崩れるのを防ぐ。
    ///
    /// 根をカーソルと同じ深さに置くので、
    /// 深さが飛ぶことはない。
    fn paste(&mut self, before: bool, count: usize) {
        let register = if self.source_mode {
            self.source_register.clone()
        } else {
            self.register.clone()
        };

        if register.is_empty() {
            self.message =
                "何もヤンクしていません".to_string();
            return;
        }

        // 必ず次の行に、カーソルと同じ深さで入れる。
        //
        // 平坦な深さのリストでは「すぐ次の行」
        // 「同じ深さ」「子を奪わない」は両立しない。
        // 続く深い行は貼ったノードのものになる。
        let depth = self.nodes[self.cursor].depth;

        // 中身も子も無い ◦ は空の置き場なので、
        // 下に足さずその場を明け渡す。
        let replace = self.text().is_empty()
            && self.children(self.cursor).is_empty();

        let at = if replace || before {
            self.cursor
        } else {
            self.cursor + 1
        };

        let mut block = Vec::new();

        for _ in 0..count {
            for node in &register {
                block.push(Node {
                    text: node.text.clone(),
                    depth: node.depth + depth,
                });
            }
        }

        let length = block.len();

        self.nodes.splice(at..at, block);

        if replace {
            self.nodes.remove(at + length);
        }

        self.cursor = at;
        self.cursor_col = 0;
        self.record();
    }

    // ------------------------------------------------------------
    // Tree表示
    // ------------------------------------------------------------

    fn tree_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();

        for index in 0..self.nodes.len() {
            let prefix = self.tree_prefix(index);

            let text = &self.nodes[index].text;

            // ソース表示では空行はそのまま空行。
            // ◦への置き換えは木の表示だけの都合。
            let display = if text.is_empty()
                && !self.source_mode
            {
                "◦"
            } else {
                text
            };

            lines.push(format!("{}{}", prefix, display));
        }

        lines
    }

    /// 描画用の行。カーソルの1セルを反転させ、
    /// h_scrollとwidthに合わせて横に切り詰める。
    ///
    /// カーソルの記号を挿し込むと桁がずれるので、
    /// 文字数を変えずに見せる。見切れた行には
    /// 端に印を出す（vimのnowrap + listcharsに近い）。
    fn tree_display(&self) -> Vec<Line<'static>> {
        let highlight = Style::default()
            .add_modifier(Modifier::REVERSED);

        let dim = Style::default()
            .add_modifier(Modifier::DIM);

        let gutter_width = number_width(self.nodes.len());

        // draw()を通す前（テストなど）はwidthが0のまま
        // なので、無制限として扱う。height==0のときの
        // follow_cursor()と同じ考え方。ソース表示は
        // 折り返すので、そもそも横スクロールしない。
        let width = if self.width == 0 || self.source_mode
        {
            usize::MAX
        } else {
            self.width
        };

        self.tree_lines()
            .into_iter()
            .enumerate()
            .map(|(index, line)| {
                // 行番号は独立したSpanにする。
                // 本文に混ぜるとカーソルの位置や
                // 横スクロールの桁がずれる。
                let mut spans = Vec::new();
                if self.number {
                    spans.push(number_span(
                        index + 1,
                        gutter_width,
                    ));
                }

                let chars: Vec<char> =
                    line.chars().collect();

                // カーソル行だけ、反転させる文字位置を
                // 求める。行末より右なら仮想の空白を
                // 1つ足した位置になる。
                let cursor_at = (index == self.cursor)
                    .then(|| self.cursor_column(index));

                let total = match cursor_at {
                    Some(at) if at >= chars.len() => {
                        at + 1
                    }
                    _ => chars.len(),
                };

                let left_more =
                    self.h_scroll > 0 && total > 0;

                let budget = width
                    .saturating_sub(left_more as usize);

                let start = self.h_scroll.min(total);
                let mut end = start
                    .saturating_add(budget)
                    .min(total);

                let right_more = end < total;

                if right_more && end > start {
                    end -= 1;
                }

                if left_more {
                    spans.push(Span::styled("‹", dim));
                }

                for i in start..end {
                    let c = chars
                        .get(i)
                        .copied()
                        .unwrap_or(' ');

                    let style =
                        if cursor_at == Some(i) {
                            highlight
                        } else {
                            Style::default()
                        };

                    spans.push(Span::styled(
                        c.to_string(),
                        style,
                    ));
                }

                if right_more {
                    spans.push(Span::styled("›", dim));
                }

                Line::from(spans)
            })
            .collect()
    }

    fn tree_prefix(&self, index: usize) -> String {
        let depth = self.nodes[index].depth;

        if depth == 0 {
            return String::new();
        }

        let mut prefix = String::new();

        for level in 0..depth {
            if level == depth - 1 {
                if self.is_last_sibling(index) {
                    prefix.push_str("└── ");
                } else {
                    prefix.push_str("├── ");
                }
            } else {
                let ancestor =
                    self.find_ancestor(index, level);

                if self.has_next_sibling(ancestor) {
                    prefix.push_str("│   ");
                } else {
                    prefix.push_str("    ");
                }
            }
        }

        prefix
    }

    /// indexの祖先のうち、深さlevel+1のものを返す。
    ///
    /// levelは罫線の桁番号で、左端が0。
    fn find_ancestor(
        &self,
        index: usize,
        level: usize,
    ) -> usize {
        let target_depth = level + 1;

        let mut i = index;

        while i > 0 {
            i -= 1;

            if self.nodes[i].depth == target_depth {
                return i;
            }
        }

        0
    }

    fn has_next_sibling(&self, index: usize) -> bool {
        let depth = self.nodes[index].depth;

        for i in index + 1..self.nodes.len() {
            let other_depth = self.nodes[i].depth;

            if other_depth < depth {
                return false;
            }

            if other_depth == depth {
                return true;
            }
        }

        false
    }

    fn is_last_sibling(&self, index: usize) -> bool {
        !self.has_next_sibling(index)
    }

    // ------------------------------------------------------------
    // S式への変換
    // ------------------------------------------------------------

    /// 木全体を整形して書き出す。
    ///
    /// 例えば
    ///
    /// define
    ///     square
    ///         x
    ///     *
    ///         x
    ///         x
    ///
    /// を
    ///
    /// (define (square x)
    ///   (* x x))
    ///
    /// にする。
    fn to_scheme(&self) -> String {
        self.to_scheme_with_positions().0
    }

    /// to_scheme()に加えて、ノードごとの出力上の
    /// (行, 桁)も返す。桁は文字数（current_columnと
    /// 同じ簡略化）。
    ///
    /// 兄弟ごと1行に畳まれた子は、畳んだ親の位置に
    /// 埋もれて記録されない。木とソース表示を
    /// 切り替えるとき、カーソルの対応する位置を
    /// 探すのに使う。
    fn to_scheme_with_positions(
        &self,
    ) -> (String, Vec<Option<(usize, usize)>>) {
        let mut output = String::new();
        let mut positions = vec![None; self.nodes.len()];

        for index in 0..self.nodes.len() {
            if self.nodes[index].depth != 0 {
                continue;
            }
            if !output.is_empty() {
                output.push_str("\n\n");
            }
            self.write_pretty(
                index,
                &mut output,
                &mut positions,
            );
        }

        (output, positions)
    }

    /// indexの位置。記録が無ければ、畳んだ親を
    /// 見つかるまで手前へ探す。
    fn position_of(
        index: usize,
        positions: &[Option<(usize, usize)>],
    ) -> (usize, usize) {
        let mut i = index;

        loop {
            if let Some(position) = positions[i] {
                return position;
            }

            if i == 0 {
                return (0, 0);
            }

            i -= 1;
        }
    }

    /// 記録の中から、target行以下で一番近いノードを
    /// 探す。ソース表示の行番号から、対応する木の
    /// ノードへ戻るときに使う。
    fn node_at_line(
        positions: &[Option<(usize, usize)>],
        target: usize,
    ) -> usize {
        let mut best = 0;
        let mut best_line = 0;

        for (index, position) in
            positions.iter().enumerate()
        {
            let Some((line, _)) = position else {
                continue;
            };

            if *line <= target && *line >= best_line {
                best_line = *line;
                best = index;
            }
        }

        best
    }

    /// indexの直接の子を列挙する。
    fn children(&self, index: usize) -> Vec<usize> {
        let depth = self.nodes[index].depth;
        let mut result = Vec::new();
        let mut i = index + 1;
        while i < self.nodes.len() {
            let child_depth = self.nodes[i].depth;
            if child_depth <= depth {
                break;
            }
            if child_depth == depth + 1 {
                result.push(i);
            }
            // 不正な深さのノードは読み飛ばす。
            i += 1;
        }
        result
    }

    /// '(a b) の ' のように、要素を括弧無しで直接
    /// 子に持てる印かどうか。
    ///
    /// テキストが印の記号と完全一致するときだけ。
    /// '`x`（連なりがアトムに畳まれた形）のように
    /// 記号の後に文字が続くものは、この判定を通らず
    /// ただのアトムとして扱われる。#t や #\a も同様。
    fn is_marker(&self, index: usize) -> bool {
        let text = self.nodes[index].text.trim();
        matches!(
            text,
            "'" | "`" | "," | ",@" | "#;" | "#" | "#u8"
        ) || (text.starts_with('#')
            && text.ends_with('=')
            && text.len() > 2)
    }

    /// indexのS式を改行なしで書き出す。
    ///
    /// 幅に収まるかどうかの判定に使う。
    fn flat(&self, index: usize) -> String {
        let text = self.nodes[index].text.trim();
        let children = self.children(index);
        if self.is_marker(index) {
            // 子が別の印か◦なら、そのまま連結する
            // （クォートの中の入れ子の引用や、
            // '() の◦のように、その1個がマーカーの
            // 中身そのものを表す場合）。
            if children.len() == 1
                && (self.is_marker(children[0])
                    || self.nodes[children[0]]
                        .text
                        .trim()
                        .is_empty())
            {
                return format!(
                    "{}{}",
                    text,
                    self.flat(children[0])
                );
            }
            // それ以外は◦を挟まず、要素を直接並べる。
            let parts: Vec<String> = children
                .iter()
                .map(|&child| self.flat(child))
                .collect();
            return format!("{}({})", text, parts.join(" "));
        }
        if !text.is_empty() && children.is_empty() {
            return text.to_string();
        }
        let mut parts = Vec::new();
        if !text.is_empty() {
            parts.push(text.to_string());
        }
        for child in children {
            parts.push(self.flat(child));
        }
        format!("({})", parts.join(" "))
    }

    fn indent_style(&self, index: usize) -> Indent {
        let text = self.nodes[index].text.trim();
        // 名前付きlet (let loop ((i 0)) ...) は
        // 名前と束縛リストの2つを見出し行に置く。
        if text == "let"
            && self
                .children(index)
                .first()
                .is_some_and(|&child| {
                    !self.nodes[child]
                        .text
                        .trim()
                        .is_empty()
                        && self.children(child).is_empty()
                })
        {
            return Indent::Body(2);
        }
        match text {
            "define" | "lambda" | "let" | "let*"
            | "letrec" | "letrec*" | "when"
            | "unless" | "case" => Indent::Body(1),
            "do" => Indent::Body(2),
            "begin" => Indent::Body(0),
            _ => Indent::Align,
        }
    }

    /// indexのS式を整形して書き出す。
    ///
    /// 開始桁はoutputの末尾から求めるので、
    /// 呼ぶ側は字下げの空白を書いてから渡すこと。
    /// positionsにindexの開始位置を記録する。
    fn write_pretty(
        &self,
        index: usize,
        output: &mut String,
        positions: &mut Vec<Option<(usize, usize)>>,
    ) {
        let indent = current_column(output);
        let line = output.matches('\n').count();
        positions[index] = Some((line, indent));

        let text = self.nodes[index].text.trim();
        let children = self.children(index);
        if self.is_marker(index) {
            // 子が別の印か◦なら、そのまま連結する。
            if children.len() == 1
                && (self.is_marker(children[0])
                    || self.nodes[children[0]]
                        .text
                        .trim()
                        .is_empty())
            {
                output.push_str(text);
                self.write_pretty(
                    children[0],
                    output,
                    positions,
                );
                return;
            }
            // それ以外は◦を挟まず、要素を直接並べる。
            // 幅に収まらなければ第1要素の桁に揃える。
            let flat = self.flat(index);
            if children.is_empty()
                || indent + flat.chars().count() <= WIDTH
            {
                output.push_str(&flat);
                return;
            }
            output.push_str(text);
            output.push('(');
            let column = current_column(output);
            self.write_pretty(
                children[0], output, positions,
            );
            self.write_children(
                &children[1..],
                column,
                output,
                positions,
            );
            output.push(')');
            return;
        }
        let flat = self.flat(index);
        if children.is_empty()
            || (!always_break(text)
                && indent + flat.chars().count()
                    <= WIDTH)
        {
            output.push_str(&flat);
            return;
        }
        output.push('(');
        if !text.is_empty() {
            output.push_str(text);
        }
        match self.indent_style(index) {
            Indent::Body(count) => {
                let count = count.min(children.len());
                for &child in &children[..count] {
                    output.push(' ');
                    self.write_pretty(
                        child, output, positions,
                    );
                }
                self.write_children(
                    &children[count..],
                    indent + 2,
                    output,
                    positions,
                );
            }
            Indent::Align => {
                if !text.is_empty() {
                    output.push(' ');
                }
                let column = current_column(output);
                self.write_pretty(
                    children[0], output, positions,
                );
                self.write_children(
                    &children[1..],
                    column,
                    output,
                    positions,
                );
            }
        }
        output.push(')');
    }

    /// 残りの子を1行ずつcolumn桁から書き出す。
    fn write_children(
        &self,
        children: &[usize],
        column: usize,
        output: &mut String,
        positions: &mut Vec<Option<(usize, usize)>>,
    ) {
        for &child in children {
            output.push('\n');
            output.push_str(&" ".repeat(column));
            self.write_pretty(child, output, positions);
        }
    }

    // ------------------------------------------------------------
    // キー入力
    // ------------------------------------------------------------

    fn handle_key(
        &mut self,
        key: KeyEvent,
    ) -> bool {
        if key.modifiers.contains(
            KeyModifiers::CONTROL
        ) && key.code == KeyCode::Char('c')
        {
            return false;
        }

        // EscのすぐあとにキーがくるとOSが1回のreadで
        // まとめて渡すことがあり、crosstermはそれを
        // Escと後続キーではなくAlt+キー1個として解釈
        // する。このアプリはAltを使わないので、
        // Escに続けて同じキーを打ったものとして
        // 分解する。
        if key.modifiers.contains(KeyModifiers::ALT) {
            self.handle_key(KeyEvent::new(
                KeyCode::Esc,
                KeyModifiers::NONE,
            ));

            return self.handle_key(KeyEvent::new(
                key.code,
                key.modifiers & !KeyModifiers::ALT,
            ));
        }

        // crosstermは生モードで \n (0x0A) を素の
        // KeyCode::Enterではなく Ctrl+j として届ける
        // （\rだけが無条件でEnterになる）。手で
        // Ctrl+jを押したときと同じ意味だが、貼り付けた
        // 複数行のテキストが1文字ずつ届くときは改行の
        // たびにこれが起きるので、Enterとして扱う。
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && key.code == KeyCode::Char('j')
        {
            return self.handle_key(KeyEvent::new(
                KeyCode::Enter,
                key.modifiers & !KeyModifiers::CONTROL,
            ));
        }

        // コマンド入力中は矢印もF2も横取りしない。
        if self.mode == Mode::Command {
            self.handle_command(key.code);
            return !self.quit;
        }

        if self.handle_common(key.code) {
            return true;
        }

        match self.mode {
            Mode::Normal => self.handle_normal(key),
            Mode::Insert => {
                self.handle_insert(key.code)
            }
            Mode::Command => {}
        }

        !self.quit
    }

    fn handle_command(&mut self, code: KeyCode) {
        match code {
            KeyCode::Esc => {
                self.command.clear();
                self.mode = Mode::Normal;
            }
            KeyCode::Enter => {
                let line =
                    std::mem::take(&mut self.command);
                self.mode = Mode::Normal;
                self.run_command(&line);
            }
            KeyCode::Backspace => {
                // 空の状態で消すと抜ける。
                if self.command.pop().is_none() {
                    self.mode = Mode::Normal;
                }
            }
            KeyCode::Char(c) => self.command.push(c),
            _ => {}
        }
    }

    /// モードによらない操作。処理したらtrueを返す。
    fn handle_common(&mut self, code: KeyCode) -> bool {
        // 1画面送りは2行重ねる。
        let page =
            self.height.saturating_sub(2).max(1) as isize;
        match code {
            KeyCode::F(2) => {
                self.toggle_source_view()
            }
            // tmuxがCtrl-Bをプレフィックスに使うので、
            // 素通りするキーも用意しておく。
            KeyCode::PageDown => self.scroll_page(page),
            KeyCode::PageUp => self.scroll_page(-page),
            KeyCode::Up => self.move_up(),
            KeyCode::Down => self.move_down(),
            KeyCode::Left => self.move_left(),
            KeyCode::Right => self.move_right(),
            _ => return false,
        }
        true
    }

    fn handle_normal(&mut self, key: KeyEvent) {
        if key
            .modifiers
            .contains(KeyModifiers::CONTROL)
        {
            // vimと同じく1画面送りは2行重ねる。
            let page = self
                .height
                .saturating_sub(2)
                .max(1) as isize;
            let half = (self.height / 2).max(1) as isize;
            match key.code {
                KeyCode::Char('r') => self.redo(),
                KeyCode::Char('f') => {
                    self.scroll_page(page)
                }
                KeyCode::Char('b') => {
                    self.scroll_page(-page)
                }
                KeyCode::Char('d') => {
                    self.scroll_page(half)
                }
                KeyCode::Char('u') => {
                    self.scroll_page(-half)
                }
                _ => {}
            }
            return;
        }

        // Tab/Shift-Tabは部分木ごと。1行だけの
        // indent()/unindent()はInsertモードのまま。
        match key.code {
            KeyCode::Tab => {
                self.begin_edit();
                self.indent_subtree();
                return;
            }
            KeyCode::BackTab => {
                self.begin_edit();
                self.unindent_subtree();
                return;
            }
            // xと同じく1文字消すが、レジスタは
            // 汚さない。
            KeyCode::Delete => {
                let count =
                    self.count.take().unwrap_or(1);
                self.begin_edit();
                for _ in 0..count {
                    self.delete_char(false);
                }
                return;
            }
            // 0 と $ と同じ。
            KeyCode::Home => {
                self.cursor_col = 0;
                return;
            }
            KeyCode::End => {
                self.cursor_col = self.text().len();
                return;
            }
            _ => {}
        }

        let KeyCode::Char(c) = key.code else {
            if key.code == KeyCode::Esc {
                self.pending = None;
                self.count = None;
            }
            return;
        };
        // 数字は回数として溜める。
        //
        // 0は溜まっているときだけ桁として扱う。
        // そうでなければ行頭移動。
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
        // 2打鍵の1打鍵目。
        //
        // ここで回数を取り出すと 3yy の 3 が
        // y に食われてしまうので、待つだけにする。
        if self.pending.is_none()
            && matches!(c, 'd' | 'y' | '>' | '<' | 'g')
        {
            self.pending = Some(c);
            return;
        }
        let given = self.count.take();
        let count = given.unwrap_or(1);
        // dd や >> のような2打鍵の命令。
        if let Some(first) = self.pending.take() {
            match (first, c) {
                ('d', 'd') => {
                    self.begin_edit();
                    self.delete_node(count, true);
                }
                ('y', 'y') => self.yank(count),
                ('>', '>') => {
                    self.begin_edit();
                    self.indent_subtree();
                }
                ('<', '<') => {
                    self.begin_edit();
                    self.unindent_subtree();
                }
                ('g', 'g') => self.move_to(0),
                _ => {}
            }
            return;
        }
        match c {
            'i' => {
                self.begin_edit();
                self.mode = Mode::Insert;
            }
            'a' => {
                self.begin_edit();
                self.move_right();
                self.mode = Mode::Insert;
            }
            'I' => {
                self.begin_edit();
                self.cursor_col = 0;
                self.mode = Mode::Insert;
            }
            'A' => {
                self.begin_edit();
                self.cursor_col = self.text().len();
                self.mode = Mode::Insert;
            }
            'o' => {
                self.begin_edit();
                self.enter();
                self.mode = Mode::Insert;
            }
            'O' => {
                self.begin_edit();
                self.open_above();
                self.mode = Mode::Insert;
            }
            'x' => {
                self.begin_edit();
                for _ in 0..count {
                    self.delete_char(true);
                }
            }
            'p' => {
                self.begin_edit();
                self.paste(false, count);
            }
            'P' => {
                self.begin_edit();
                self.paste(true, count);
            }
            // 部分木ごと。回数は受けない。
            'Y' => {
                let length = self.subtree_len();
                self.yank(length);
            }
            'D' => {
                let length = self.subtree_len();
                self.begin_edit();
                self.delete_node(length, true);
            }
            'u' => self.undo(),
            'h' => {
                for _ in 0..count {
                    self.move_left();
                }
            }
            // scroll_byはソース表示なら画面を、
            // 木の表示ならカーソルを動かす。
            'j' => self.scroll_by(count as isize),
            'k' => self.scroll_by(-(count as isize)),
            'l' => {
                for _ in 0..count {
                    self.move_right();
                }
            }
            '0' => self.cursor_col = 0,
            '$' => self.cursor_col = self.text().len(),
            'G' => {
                // 回数があればその番号のノードへ。
                let index = match given {
                    Some(number) => number - 1,
                    None => self.nodes.len() - 1,
                };
                self.move_to(index);
            }
            'M' => {
                // 画面に見えている範囲の中央へ。
                // draw()を通す前はheightが0なので、
                // 代わりに全体の中央へ寄せる。
                let index = if self.height == 0 {
                    self.nodes.len() / 2
                } else {
                    self.scroll + self.height / 2
                };
                self.move_to(
                    index.min(self.nodes.len() - 1),
                );
            }
            ':' => {
                self.command.clear();
                self.message.clear();
                self.mode = Mode::Command;
            }
            _ => {}
        }
    }

    fn handle_insert(&mut self, code: KeyCode) {
        match code {
            KeyCode::Esc => self.mode = Mode::Normal,
            // ソース表示では罫線ではなく空白を入れる。
            KeyCode::Tab if self.source_mode => {
                self.insert_char(' ');
                self.insert_char(' ');
            }
            KeyCode::Tab => self.indent(),
            KeyCode::BackTab => self.unindent(),
            KeyCode::Enter => self.enter(),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete_char(false),
            KeyCode::Home => self.cursor_col = 0,
            KeyCode::End => {
                self.cursor_col = self.text().len()
            }
            KeyCode::Char(c) => self.insert_char(c),
            _ => {}
        }
    }
}

// ------------------------------------------------------------
// 描画
// ------------------------------------------------------------

fn draw(
    frame: &mut Frame,
    app: &mut App,
) {
    // 下1行をコマンドとメッセージに使う。
    let areas = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(frame.area());

    // 枠の上下を除いた行数。
    // 画面の高さは描画時にしか分からない。
    app.height =
        areas[0].height.saturating_sub(2) as usize;

    // 枠と行番号ガターを除いた横の文字数。
    let gutter = if app.number {
        number_width(app.nodes.len())
    } else {
        0
    };
    app.width = (areas[0].width as usize)
        .saturating_sub(2)
        .saturating_sub(gutter);

    // ソース表示も木と同じ App::nodes/cursor を使う
    // （F2でdepth:0の行に変換してある）ので、
    // カーソル追従・行番号・カーソル反転は共通。
    // 横スクロールだけソース表示では不要（折り返す）。
    app.follow_cursor();
    if !app.source_mode {
        app.follow_cursor_horizontal();
    }

    let text: Text = app.tree_display().into();

    let mut paragraph = Paragraph::new(text)
        .scroll((app.scroll as u16, 0));

    // ソース表示は折り返す。カーソル行という概念が
    // 無いので、木の表示のような横スクロールは
    // 持ち込まず素直に折り返せる。
    if app.source_mode {
        paragraph = paragraph
            .wrap(Wrap { trim: false });
    }

    let paragraph = paragraph
        .block(
            Block::default()
                .title(title(app))
                .borders(Borders::ALL),
        );

    frame.render_widget(paragraph, areas[0]);

    let status = if app.mode == Mode::Command {
        format!(":{}", app.command)
    } else {
        app.message.clone()
    };

    frame.render_widget(
        Paragraph::new(status),
        areas[1],
    );
}

/// ファイル名、変更の有無、モードを並べた見出し。
fn title(app: &App) -> String {
    let name = match &app.path {
        Some(path) => path.display().to_string(),
        None => "[無名]".to_string(),
    };

    let mark = if app.modified { " [+]" } else { "" };

    // ソース表示中も編集モードを出す。中身を見れば
    // ソース表示かどうかは分かるので、そこは出さない。
    let mode = match app.mode {
        Mode::Normal => "NORMAL",
        Mode::Insert => "INSERT",
        Mode::Command => "COMMAND",
    };

    format!(" {}{} - {} ", name, mark, mode)
}

// ------------------------------------------------------------
// main
// ------------------------------------------------------------

fn main() -> io::Result<()> {
    enable_raw_mode()?;

    let mut stdout = io::stdout();

    execute!(
        stdout,
        EnterAlternateScreen,
        EnableBracketedPaste
    )?;

    let backend =
        CrosstermBackend::new(stdout);

    let mut terminal =
        Terminal::new(backend)?;

    let path = std::env::args()
        .nth(1)
        .map(PathBuf::from);

    let result =
        run(&mut terminal, path);

    disable_raw_mode()?;

    execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        LeaveAlternateScreen
    )?;

    terminal.show_cursor()?;

    result
}

fn run(
    terminal: &mut Terminal<
        CrosstermBackend<io::Stdout>
    >,
    path: Option<PathBuf>,
) -> io::Result<()> {
    let mut app = App::new();

    if let Some(path) = path {
        if path.exists() {
            app.load(&path);
        }
        app.path = Some(path);
    }

    loop {
        terminal.draw(|frame| {
            draw(frame, &mut app);
        })?;

        if event::poll(
            Duration::from_millis(50)
        )? {
            match event::read()? {
                Event::Key(key) => {
                    // 離したときのイベントを送る端末が
                    // あるので、押した分だけ処理する。
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }
                    if !app.handle_key(key) {
                        break;
                    }
                }
                Event::Paste(text) => {
                    app.handle_paste(&text);
                }
                _ => {}
            }
        }
    }

    Ok(())
}



// ------------------------------------------------------------
// テスト
// ------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// キー列をhandle_keyに流し込み、TUIと同じ経路を辿る。
    ///
    /// `\t` はTab、`\n` はEnter、`\x08` はShift-Tab、
    /// `\x1b` はEsc、`\x7f` はBackspace、
    /// `\x04` はDelete。
    fn press(app: &mut App, keys: &str) {
        for key in keys.chars() {
            let code = match key {
                '\t' => KeyCode::Tab,
                '\n' => KeyCode::Enter,
                '\x08' => KeyCode::BackTab,
                '\x1b' => KeyCode::Esc,
                '\x7f' => KeyCode::Backspace,
                '\x04' => KeyCode::Delete,
                _ => KeyCode::Char(key),
            };
            app.handle_key(KeyEvent::new(
                code,
                KeyModifiers::NONE,
            ));
        }
    }

    /// Normalモードで始まるので、iを打ってから流す。
    fn insert(keys: &str) -> App {
        let mut app = App::new();
        press(&mut app, "i");
        press(&mut app, keys);
        app
    }

    /// キー列を打ってからF2の出力を得る。
    fn scheme(keys: &str) -> String {
        insert(keys).to_scheme()
    }

    /// キー列を打ってからカーソル表示を除いた木を得る。
    fn tree(keys: &str) -> String {
        insert(keys).tree_lines().join("\n")
    }

    fn check(cases: &[(&str, &str)]) {
        for (keys, expected) in cases {
            assert_eq!(
                scheme(keys),
                *expected,
                "\nキー列: {:?}",
                keys
            );
        }
    }

    /// データの表記。
    #[test]
    fn notation() {
        check(&[
            // 空リストとアトム
            ("", "()"),
            ("x", "x"),
            ("f\n\t", "(f ())"),
            // テキストがcar、子がcdr
            ("f\n\ta", "(f a)"),
            ("+\n\t1\n2", "(+ 1 2)"),
            // 先頭にシンボルを持たないリスト
            ("\n\ta\nb", "(a b)"),
            // ドット対
            ("a\n\t.\nb", "(a . b)"),
            ("f\n\ta\n.\nrest", "(f a . rest)"),
        ]);
    }

    /// 印ノード。要素は◦を挟まず直接子に持つ。
    #[test]
    fn markers() {
        check(&[
            ("'\n\ta\nb", "'(a b)"),
            ("`\n\ta\n,b", "`(a ,b)"),
            (",@\n\ta\nb", ",@(a b)"),
            ("#\n\t1\n2", "#(1 2)"),
            ("#u8\n\t1\n2", "#u8(1 2)"),
            ("#;\n\ta\nb", "#;(a b)"),
            ("#0=\n\ta\nb", "#0=(a b)"),
            // 3個以上も同様。
            ("'\n\t1\n2\n3", "'(1 2 3)"),
            // 要素1個はマーカーが直接持つ。
            ("'\n\tx", "'(x)"),
            // 空リストは◦を挟む例外。
            ("'\n\t", "'()"),
            // 入れ子の擬似引用。
            (
                "`\n\ta\n`\n\tb\n,c",
                "`(a `(b ,c))",
            ),
            // 引用付きシンボルはただのテキスト。
            ("\n\t'a\nb", "('a b)"),
        ]);
    }

    /// #で始まるが子を持たないものはアトム。
    #[test]
    fn hash_atoms() {
        check(&[
            ("#t", "#t"),
            ("list\n\t#t\n#f\n#\\a", "(list #t #f #\\a)"),
            ("#!fold-case", "#!fold-case"),
        ]);
    }

    /// 本体インデント型。短くても必ず改行する。
    #[test]
    fn body_forms() {
        check(&[
            (
                "define\n\tsquare\n\tx\n\x08*\n\tx\nx",
                "(define (square x)\n  (* x x))",
            ),
            (
                "lambda\n\t\n\tx\n\x08*\n\tx\nx",
                "(lambda (x)\n  (* x x))",
            ),
            ("when\n\ta\nb\nc", "(when a\n  b\n  c)"),
            ("begin\n\ta\nb", "(begin\n  a\n  b)"),
            (
                "let\n\t\n\tx\n\t1\n\x08y\n\t2\n\x08\x08body",
                "(let ((x 1) (y 2))\n  body)",
            ),
            (
                "let\n\tloop\n\n\ti\n\t0\n\x08\x08body",
                "(let loop ((i 0))\n  body)",
            ),
            (
                "do\n\t\n\ti\n\t0\n\x08\x08\n\t=\n\ti\n5\n\x08\x08body",
                "(do ((i 0)) ((= i 5))\n  body)",
            ),
            (
                "case\n\tx\n\n\t1\n\t2\n\x08'a\n\x08\n\telse\n'b",
                "(case x\n  ((1 2) 'a)\n  (else 'b))",
            ),
        ]);
    }

    /// 整列型。子を第1子の桁に揃える。
    #[test]
    fn align_forms() {
        check(&[
            (
                "cond\n\t\n\t<\n\tx\n2\n\x081\n\x08\n\t>\n\tx\n2\n\x08-1",
                "(cond ((< x 2) 1)\n      ((> x 2) -1))",
            ),
            (
                "cond\n\t\n\tassv\n\tk\nal\n\x08=>\ncdr\n\x08\n\telse\n#f",
                "(cond ((assv k al) => cdr)\n      (else #f))",
            ),
            // 幅に収まるので1行
            ("and\n\ta\nb", "(and a b)"),
            ("or\n\ta\nb", "(or a b)"),
            // 幅を超えるので第1引数の桁で折り返す
            (
                "some-quite-long-function-name\n\
                 \taaaaaaaaaaaaaaaaaaaaaaaaaaa\n\
                 bbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "(some-quite-long-function-name \
                 aaaaaaaaaaaaaaaaaaaaaaaaaaa\n\
                 \x20                              \
                 bbbbbbbbbbbbbbbbbbbbbbbbbbb)",
            ),
        ]);
    }

    /// トップレベルの式は空行で区切る。
    #[test]
    fn toplevel() {
        check(&[
            ("a\nb", "a\n\nb"),
            (
                "define\n\ta\n1\n\x08define\n\tb\n2",
                "(define a\n  1)\n\n(define b\n  2)",
            ),
        ]);
    }

    /// 編集で作れる縮退した形でも、印字は落ちずに
    /// それなりの結果を返す。
    #[test]
    fn marker_degenerate_shapes() {
        check(&[
            // 子を1つも作らずに終わった印。
            ("'", "'()"),
            // 子が1個で、それ自体が子を持つ複合式
            // （'a に子bを付けた場合）。1要素のリストの
            // 中身がさらにリストである、という意味になる。
            ("'\n\ta\n\tb", "'((a b))"),
        ]);
    }

    /// 深い木の罫線。
    #[test]
    fn tree_view() {
        assert_eq!(
            tree("cond\n\t\n\t<\n\tx\n2\n\x081\n\x08\n\t>\n\tx\n2\n\x08-1"),
            "cond\n\
             ├── ◦\n\
             │   ├── <\n\
             │   │   ├── x\n\
             │   │   └── 2\n\
             │   └── 1\n\
             └── ◦\n\
             \x20   ├── >\n\
             \x20   │   ├── x\n\
             \x20   │   └── 2\n\
             \x20   └── -1"
        );
        assert_eq!(
            tree("`\n\ta\n\t`\n\tb\n\t,c"),
            "`\n\
             └── a\n\
             \x20   └── `\n\
             \x20       └── b\n\
             \x20           └── ,c"
        );
        assert_eq!(
            tree("let\n\t\n\tx\n\t1\n\x08y\n\t2\n\x08\x08body"),
            "let\n\
             ├── ◦\n\
             │   ├── x\n\
             │   │   └── 1\n\
             │   └── y\n\
             │       └── 2\n\
             └── body"
        );
    }

    /// Normalモードの操作。
    #[test]
    fn normal_mode() {
        // Escで抜けてから打った文字は命令として効く。
        // ddはノードだけ消して子を持ち上げる。
        let mut app = insert("f\n\ta\n\tb");
        press(&mut app, "\x1b");
        press(&mut app, "kkdd");
        assert_eq!(app.to_scheme(), "(a b)");
        // >> と << で階層を変える。
        let mut app = insert("f\na");
        press(&mut app, "\x1b>>");
        assert_eq!(app.to_scheme(), "(f a)");
        press(&mut app, "<<");
        assert_eq!(app.to_scheme(), "f\n\na");
        // o は下に、O は上に空ノードを作って挿入モードへ。
        let mut app = insert("a");
        press(&mut app, "\x1bob");
        assert_eq!(app.to_scheme(), "a\n\nb");
        let mut app = insert("a");
        press(&mut app, "\x1bOb");
        assert_eq!(app.to_scheme(), "b\n\na");
        // x は1文字消す。0 と $ は行頭と行末。
        let mut app = insert("abc");
        press(&mut app, "\x1b0x");
        assert_eq!(app.to_scheme(), "bc");
        let mut app = insert("abc");
        press(&mut app, "\x1b0$icut-");
        assert_eq!(app.to_scheme(), "abccut-");
        // gg と G で先頭と末尾へ。
        let mut app = insert("a\nb\nc");
        press(&mut app, "\x1bggIX");
        assert_eq!(app.to_scheme(), "Xa\n\nb\n\nc");
        press(&mut app, "\x1bGA!");
        assert_eq!(app.to_scheme(), "Xa\n\nb\n\nc!");
    }

    /// カーソルは反転セルで示す。桁はずれない。
    #[test]
    fn cursor_cell() {
        // 反転しているセルの中身。
        //
        // ガターや見切れの印が増えると位置が動くので、
        // 固定indexではなくスタイルで探す。
        fn cell(app: &App) -> String {
            app.tree_display()[app.cursor]
                .spans
                .iter()
                .find(|span| {
                    span.style.add_modifier
                        .contains(Modifier::REVERSED)
                })
                .expect("カーソルのセルが見つかりません")
                .content
                .to_string()
        }
        // 行の中身は反転の有無で変わらない。
        fn line(app: &App) -> String {
            app.tree_display()[app.cursor]
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        }
        let mut app = insert("abc");
        press(&mut app, "\x1b0");
        assert_eq!(cell(&app), "a");
        assert_eq!(line(&app), "abc");
        press(&mut app, "l");
        assert_eq!(cell(&app), "b");
        assert_eq!(line(&app), "abc");
        // 行末より右では空白を反転する。
        press(&mut app, "$");
        assert_eq!(cell(&app), " ");
        assert_eq!(line(&app), "abc ");
        // 空ノードは◦の上。
        let app = App::new();
        assert_eq!(cell(&app), "◦");
        assert_eq!(line(&app), "◦");
    }

    /// 多バイト文字。列はバイト位置で持っている。
    #[test]
    fn multibyte() {
        let mut app = insert("\"日本語\"");
        assert_eq!(app.to_scheme(), "\"日本語\"");
        // hで閉じ引用符と語を戻り、xで語を消す。
        press(&mut app, "\x1bhhx");
        assert_eq!(app.to_scheme(), "\"日本\"");
        // 挿入モードのBackspaceも1文字単位。
        press(&mut app, "i\x7f");
        assert_eq!(app.to_scheme(), "\"日\"");
    }

    /// :コマンド。
    #[test]
    fn commands() {
        let path = std::env::temp_dir()
            .join("seditor-commands.scm");
        let _ = fs::remove_file(&path);
        let name = path.display().to_string();
        // 編集するとmodifiedが立つ。
        let mut app = insert("f\n\ta");
        press(&mut app, "\x1b");
        assert!(app.modified);
        // ファイル名が無ければ書けない。
        press(&mut app, ":w\n");
        assert_eq!(app.message, "ファイル名がありません");
        assert!(app.modified);
        // 名前を渡せば書けて、以降は覚えている。
        press(&mut app, &format!(":w {}\n", name));
        assert!(!app.modified);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "(f a)\n"
        );
        assert_eq!(app.path, Some(path.clone()));
        // 未保存の変更があると :q は断る。
        press(&mut app, "ib\x1b");
        assert!(app.modified);
        assert!(app.handle_key(KeyEvent::new(
            KeyCode::Char(':'),
            KeyModifiers::NONE
        )));
        press(&mut app, "q\n");
        assert!(!app.quit);
        assert!(app.message.contains(":q!"));
        // :q! は捨てて終わる。
        press(&mut app, ":q!\n");
        assert!(app.quit);
        // Escで取り消せる。
        let mut app = insert("a");
        press(&mut app, "\x1b:q!\x1b");
        assert!(!app.quit);
        assert_eq!(app.command, "");
        // 知らないコマンド。
        press(&mut app, ":zzz\n");
        assert_eq!(
            app.message,
            "不明なコマンドです: zzz"
        );
        // :wq は書いてから終わる。
        let mut app = insert("x");
        press(&mut app, &format!("\x1b:wq {}\n", name));
        assert!(app.quit);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "x\n"
        );
        let _ = fs::remove_file(&path);
    }

    /// 読んで印刷し直すと元に戻る。
    ///
    /// 印刷側と読み取り側の食い違いはここで出る。
    fn roundtrip(text: &str) -> String {
        let reading = reader::read(text).unwrap();
        let mut app = App::new();
        app.nodes = nodes_from(&reading.data);
        app.to_scheme()
    }

    fn assert_stable(text: &str) {
        assert_eq!(roundtrip(text), text, "\n1回目");
        // 2回目以降変わらないこと。
        assert_eq!(
            roundtrip(&roundtrip(text)),
            text,
            "\n2回目"
        );
    }

    /// 印刷したものを読み戻すと同じになる。
    #[test]
    fn read_back() {
        for text in [
            "x",
            "()",
            "(f a)",
            "(f ())",
            "(f)",
            "(a . b)",
            "(f a . rest)",
            "'x",
            "'`x",
            "'()",
            "'(x)",
            "'(a b)",
            "'(1 2 3)",
            "'(a (b c) d)",
            "`(a ,b)",
            ",@(a b)",
            "#(1 2)",
            "#u8(1 2)",
            "#;(a b)",
            "#0=(a b)",
            "`(a `(b ,c))",
            "('a b)",
            "(list #t #f #\\a)",
            "\"日本語\"",
            "(f \"a b\" |c d|)",
            "(define (square x)\n  (* x x))",
            "(cond ((< x 2) 1)\n      ((> x 2) -1))",
            "(let ((x 1) (y 2))\n  body)",
            "(let loop ((i 0))\n  body)",
            "(do ((i 0)) ((= i 5))\n  body)",
            "(case x\n  ((1 2) 'a)\n  (else 'b))",
            "(when a\n  b\n  c)",
            "(begin\n  a\n  b)",
            "(lambda (x)\n  (* x x))",
            "(and a b)",
            "a\n\nb",
            "; head\n\n(define a\n  1)",
        ] {
            assert_stable(text);
        }
    }

    /// 式の中のコメントは行頭に出る。
    #[test]
    fn hoisted_comments() {
        let reading =
            reader::read("(a ; note\n b)").unwrap();
        assert_eq!(reading.hoisted, 1);
        let mut app = App::new();
        app.nodes = nodes_from(&reading.data);
        assert_eq!(app.to_scheme(), "; note\n\n(a b)");
    }

    /// :e でファイルを読む。
    #[test]
    fn edit_command() {
        let path = std::env::temp_dir()
            .join("seditor-edit.scm");
        let name = path.display().to_string();
        fs::write(&path, "(define (f x)\n  (+ x 1))\n")
            .unwrap();
        let mut app = App::new();
        press(&mut app, &format!(":e {}\n", name));
        assert_eq!(
            app.to_scheme(),
            "(define (f x)\n  (+ x 1))"
        );
        assert!(!app.modified);
        assert_eq!(app.path, Some(path.clone()));
        // 未保存の変更があると断る。
        press(&mut app, "ib\x1b:e\n");
        assert!(app.message.contains(":e!"));
        // :e! なら捨てて読み直す。
        press(&mut app, ":e!\n");
        assert_eq!(
            app.to_scheme(),
            "(define (f x)\n  (+ x 1))"
        );
        // 壊れたファイルは読まない。
        fs::write(&path, "(a").unwrap();
        press(&mut app, ":e!\n");
        assert!(app.message.contains("括弧が閉じていません"));
        let _ = fs::remove_file(&path);
    }

    /// アンドゥとリドゥ。
    #[test]
    fn undo_redo() {
        // 挿入セッション全体が1段。
        let mut app = insert("abc");
        press(&mut app, "\x1b");
        assert_eq!(app.to_scheme(), "abc");
        press(&mut app, "u");
        assert_eq!(app.to_scheme(), "()");
        assert_eq!(app.undo.len(), 0);
        // Ctrl-rで戻る。
        app.handle_key(KeyEvent::new(
            KeyCode::Char('r'),
            KeyModifiers::CONTROL,
        ));
        assert_eq!(app.to_scheme(), "abc");
        // 何も打たずに抜けた挿入は段を作らない。
        let mut app = insert("a");
        press(&mut app, "\x1b");
        let depth = app.undo.len();
        press(&mut app, "i\x1b");
        assert_eq!(app.undo.len(), depth);
        // 空振りのxも段を作らない。
        press(&mut app, "$x");
        assert_eq!(app.undo.len(), depth);
        // Normalの編集命令はそれぞれ1段。
        let mut app = insert("f\n\ta\nb");
        press(&mut app, "\x1b");
        assert_eq!(app.to_scheme(), "(f a b)");
        press(&mut app, "dd");
        assert_eq!(app.to_scheme(), "(f a)");
        press(&mut app, "<<");
        assert_eq!(app.to_scheme(), "f\n\na");
        press(&mut app, "uu");
        assert_eq!(app.to_scheme(), "(f a b)");
        // 戻ったあとに編集するとリドゥは消える。
        press(&mut app, "dd");
        app.handle_key(KeyEvent::new(
            KeyCode::Char('r'),
            KeyModifiers::CONTROL,
        ));
        assert_eq!(app.to_scheme(), "(f a)");
        assert!(app.redo.is_empty());
        // カーソルも戻る。
        let mut app = insert("abc\nx");
        press(&mut app, "\x1bdd");
        assert_eq!(app.cursor, 0);
        press(&mut app, "u");
        assert_eq!(app.cursor, 1);
        // 端では断る。
        let mut app = App::new();
        press(&mut app, "u");
        assert_eq!(app.message, "これ以上戻れません");
        app.handle_key(KeyEvent::new(
            KeyCode::Char('r'),
            KeyModifiers::CONTROL,
        ));
        assert_eq!(app.message, "これ以上やり直せません");
    }

    /// :e は履歴を捨てる。
    #[test]
    fn edit_clears_history() {
        let path = std::env::temp_dir()
            .join("seditor-undo.scm");
        fs::write(&path, "(a b)\n").unwrap();
        let mut app = insert("x");
        press(&mut app, "\x1b");
        assert!(!app.undo.is_empty());
        press(
            &mut app,
            &format!(":e! {}\n", path.display()),
        );
        assert_eq!(app.to_scheme(), "(a b)");
        assert!(app.undo.is_empty());
        assert!(app.redo.is_empty());
        let _ = fs::remove_file(&path);
    }

    /// ◦ の上では x も Backspace も Delete も
    /// ノードごと消す。
    #[test]
    fn delete_on_empty_node() {
        // x はddと同じく子を1段持ち上げる。
        let mut app = insert("f\n\t\n\ta");
        press(&mut app, "\x1b");
        assert_eq!(app.to_scheme(), "(f (a))");
        press(&mut app, "kx");
        assert_eq!(app.to_scheme(), "(f a)");
        // 挿入モードのDeleteも同じ。
        let mut app = insert("f\n\t");
        assert_eq!(app.to_scheme(), "(f ())");
        press(&mut app, "\x04");
        assert_eq!(app.to_scheme(), "f");
        // Backspaceは前のノードの末尾に戻る。
        let mut app = insert("abc\n\t");
        assert_eq!(app.to_scheme(), "(abc ())");
        press(&mut app, "\x7f");
        assert_eq!(app.to_scheme(), "abc");
        assert_eq!(app.cursor, 0);
        assert_eq!(app.cursor_col, 3);
        // 挿入モードのままなので続けて打てる。
        press(&mut app, "d");
        assert_eq!(app.to_scheme(), "abcd");
        // 末尾の ◦ を消しても2つ上に飛ばない。
        let mut app = insert("a\nb\n\t");
        assert_eq!(app.to_scheme(), "a\n\n(b ())");
        press(&mut app, "\x7f");
        assert_eq!(app.to_scheme(), "a\n\nb");
        assert_eq!(app.cursor, 1);
        assert_eq!(app.cursor_col, 1);
        // 先頭の ◦ では前に戻らない。
        let mut app = insert("\nx");
        press(&mut app, "\x1bkk");
        assert_eq!(app.cursor, 0);
        press(&mut app, "i\x7f");
        assert_eq!(app.to_scheme(), "x");
        assert_eq!(app.cursor_col, 0);
        // 消したぶんは u で戻る。
        press(&mut app, "\x1bu");
        assert_eq!(app.to_scheme(), "()\n\nx");
    }

    /// ヤンクと貼り付け。
    #[test]
    fn yank_paste() {
        // yy はノード1つ。p は次の兄弟に貼る。
        let mut app = insert("f\n\ta\nb");
        assert_eq!(app.to_scheme(), "(f a b)");
        press(&mut app, "\x1bkyy");
        assert_eq!(app.message, "1ノードをヤンクしました");
        press(&mut app, "p");
        assert_eq!(app.to_scheme(), "(f a a b)");
        assert_eq!(app.cursor, 2);
        // P は手前に貼る。
        press(&mut app, "P");
        assert_eq!(app.to_scheme(), "(f a a a b)");
        // 回数を付けて貼る。
        let mut app = insert("f\n\ta");
        press(&mut app, "\x1byy2p");
        assert_eq!(app.to_scheme(), "(f a a a)");
        // 3yy は連続3ノードを相対の深さごと取る。
        let mut app = insert("f\n\ta\n\tb\n\x08c");
        press(&mut app, "\x1b");
        assert_eq!(app.to_scheme(), "(f (a b) c)");
        press(&mut app, "gg3yyGp");
        assert_eq!(
            app.to_scheme(),
            "(f (a b) c (f (a b)))"
        );
        // 何もヤンクしていなければ断る。
        let mut app = insert("a");
        press(&mut app, "\x1bp");
        assert_eq!(app.message, "何もヤンクしていません");
    }

    /// 中身も子も無い ◦ は貼り付けで置き換わる。
    #[test]
    fn paste_replaces_empty_node() {
        let mut app = insert("f\n\ta\n");
        assert_eq!(app.to_scheme(), "(f a ())");
        press(&mut app, "\x1bkyyjp");
        assert_eq!(app.to_scheme(), "(f a a)");
        assert_eq!(app.cursor, 2);
        // 子を持つ ◦ は置き換えない。
        let mut app = insert("f\n\ta\n\n\tb");
        press(&mut app, "\x1b");
        assert_eq!(app.to_scheme(), "(f a (b))");
        press(&mut app, "ggjyyjp");
        assert_eq!(app.to_scheme(), "(f a () (a b))");
    }

    /// p は次の行にカーソルと同じ深さで割り込む。
    #[test]
    fn paste_interrupts() {
        // 葉の上では兄弟になる。
        let mut app = insert("f\n\ta\nb");
        press(&mut app, "\x1b");
        assert_eq!(app.to_scheme(), "(f a b)");
        press(&mut app, "kyyp");
        assert_eq!(app.to_scheme(), "(f a a b)");
        assert_eq!(app.cursor, 2);
        // 子を持つノードの上では、続く深い行が
        // 貼ったノードのものになる。
        let mut app = insert("f\n\ta\n\tb");
        press(&mut app, "\x1b");
        assert_eq!(app.to_scheme(), "(f (a b))");
        press(&mut app, "yykp");
        assert_eq!(app.to_scheme(), "(f a (b b))");
    }

    /// 回数指定。
    #[test]
    fn counts() {
        // 移動。
        let mut app = insert("a\nb\nc\nd");
        press(&mut app, "\x1bgg2j");
        assert_eq!(app.cursor, 2);
        press(&mut app, "2k");
        assert_eq!(app.cursor, 0);
        // 3G は3番目のノード。G だけなら末尾。
        press(&mut app, "3G");
        assert_eq!(app.cursor, 2);
        press(&mut app, "G");
        assert_eq!(app.cursor, 3);
        // 2桁も溜まる。
        press(&mut app, "gg10j");
        assert_eq!(app.cursor, 3);
        // 0は溜まっていなければ行頭移動。
        let mut app = insert("abc");
        press(&mut app, "\x1b0");
        assert_eq!(app.cursor_col, 0);
        assert_eq!(app.count, None);
        // 2x は2文字消す。
        press(&mut app, "2x");
        assert_eq!(app.to_scheme(), "c");
        // Escで回数を捨てる。
        press(&mut app, "3\x1b");
        assert_eq!(app.count, None);
    }

    /// まとめて消すと深さが飛ぶので復元する。
    #[test]
    fn delete_repairs_depth() {
        // a(0) b(1) c(2) d(3) から b c を消すと
        // a(0) d(3) が残り、深さが0から3に飛ぶ。
        let mut app = insert("a\n\tb\n\tc\n\td");
        press(&mut app, "\x1b");
        assert_eq!(app.to_scheme(), "(a (b (c d)))");
        press(&mut app, "ggj2dd");
        assert_eq!(app.to_scheme(), "(a d)");
        assert_eq!(app.nodes[1].depth, 1);
        // 消したぶんはレジスタに入る。
        press(&mut app, "p");
        assert_eq!(app.to_scheme(), "(a d (b c))");
    }

    /// スクロールとページ送り。
    #[test]
    fn scrolling() {
        // 20ノードを高さ5の画面で見る。
        let mut app = App::new();
        press(&mut app, "i0");
        for n in 1..20 {
            press(&mut app, &format!("\n{}", n));
        }
        press(&mut app, "\x1b");
        assert_eq!(app.nodes.len(), 20);
        app.height = 5;
        // 見えている間は動かない。
        app.cursor = 0;
        app.follow_cursor();
        assert_eq!(app.scroll, 0);
        app.cursor = 4;
        app.follow_cursor();
        assert_eq!(app.scroll, 0);
        // 下に外れたら最小限だけ送る。
        app.cursor = 5;
        app.follow_cursor();
        assert_eq!(app.scroll, 1);
        // 上に外れたらその行を最上部に。
        app.cursor = 0;
        app.follow_cursor();
        assert_eq!(app.scroll, 0);
        // Ctrl-F は1画面分、Ctrl-D は半画面分。
        let page = |app: &mut App, c: char| {
            app.handle_key(KeyEvent::new(
                KeyCode::Char(c),
                KeyModifiers::CONTROL,
            ));
        };
        // 高さ5なら1画面は3行（2行重ねる）、
        // 半画面は2行。画面もカーソルも同じだけ動く。
        page(&mut app, 'f');
        assert_eq!((app.scroll, app.cursor), (3, 3));
        page(&mut app, 'd');
        assert_eq!((app.scroll, app.cursor), (5, 5));
        page(&mut app, 'b');
        assert_eq!((app.scroll, app.cursor), (2, 2));
        page(&mut app, 'u');
        assert_eq!((app.scroll, app.cursor), (0, 0));
        // 端では止まる。
        page(&mut app, 'b');
        assert_eq!(app.cursor, 0);
        for _ in 0..10 {
            page(&mut app, 'f');
        }
        assert_eq!(app.cursor, 19);
    }

    /// 実際に描画して画面に見える行を返す。
    fn screen(
        terminal: &mut Terminal<
            ratatui::backend::TestBackend,
        >,
        app: &mut App,
    ) -> Vec<String> {
        terminal
            .draw(|frame| draw(frame, app))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let mut lines = Vec::new();
        // 枠の上下を除いた行だけを見る。
        for y in 1..buffer.area.height - 2 {
            let mut line = String::new();
            for x in 1..buffer.area.width - 1 {
                line.push_str(buffer[(x, y)].symbol());
            }
            let line = line.trim_end().to_string();
            if !line.is_empty() {
                lines.push(line);
            }
        }
        lines
    }

    /// 実際に描画してページ送りを確かめる。
    ///
    /// heightを手で設定するテストでは描画経路を
    /// 通らないので、画面に見える内容で確かめる。
    #[test]
    fn paging_on_screen() {
        // 高さ10。枠2行と下1行を引いて7行見える。
        let mut terminal = Terminal::new(
            ratatui::backend::TestBackend::new(20, 10),
        )
        .unwrap();
        let mut app = App::new();
        press(&mut app, "i0");
        for n in 1..20 {
            press(&mut app, &format!("\n{}", n));
        }
        press(&mut app, "\x1bgg");
        let ctrl = |app: &mut App, c: char| {
            app.handle_key(KeyEvent::new(
                KeyCode::Char(c),
                KeyModifiers::CONTROL,
            ));
        };
        let top = |t: &mut Terminal<_>, app: &mut App| {
            screen(t, app)[0].clone()
        };
        assert_eq!(top(&mut terminal, &mut app), "0");
        assert_eq!(app.height, 7);
        // 最上部では戻れない。
        ctrl(&mut app, 'b');
        assert_eq!(top(&mut terminal, &mut app), "0");
        // 1画面は2行重ねて5行。初回から画面が動く。
        ctrl(&mut app, 'f');
        assert_eq!(top(&mut terminal, &mut app), "5");
        ctrl(&mut app, 'f');
        assert_eq!(top(&mut terminal, &mut app), "10");
        ctrl(&mut app, 'b');
        assert_eq!(top(&mut terminal, &mut app), "5");
        // 半画面は3行。
        ctrl(&mut app, 'd');
        assert_eq!(top(&mut terminal, &mut app), "8");
        ctrl(&mut app, 'u');
        assert_eq!(top(&mut terminal, &mut app), "5");
        // 最下部より先には送らない。
        for _ in 0..10 {
            ctrl(&mut app, 'f');
        }
        assert_eq!(top(&mut terminal, &mut app), "13");
        assert_eq!(app.cursor, 19);
        // jで下端を越えると1行ずつ付いてくる。
        press(&mut app, "gg");
        assert_eq!(top(&mut terminal, &mut app), "0");
        for _ in 0..7 {
            press(&mut app, "j");
        }
        assert_eq!(top(&mut terminal, &mut app), "1");
        // ソース表示も同じ仕組みで送れる。
        app.toggle_source_view();
        screen(&mut terminal, &mut app);
        ctrl(&mut app, 'f');
        assert!(app.scroll > 0);
    }

    /// PageUp / PageDown。
    ///
    /// tmuxがCtrl-Bをプレフィックスに使うため、
    /// 素通りするキーでも送れる必要がある。
    #[test]
    fn page_keys() {
        let mut terminal = Terminal::new(
            ratatui::backend::TestBackend::new(20, 10),
        )
        .unwrap();
        let mut app = App::new();
        press(&mut app, "i0");
        for n in 1..20 {
            press(&mut app, &format!("\n{}", n));
        }
        press(&mut app, "\x1bgg");
        let key = |app: &mut App, code: KeyCode| {
            app.handle_key(KeyEvent::new(
                code,
                KeyModifiers::NONE,
            ));
        };
        let top = |t: &mut Terminal<_>, app: &mut App| {
            screen(t, app)[0].clone()
        };
        assert_eq!(top(&mut terminal, &mut app), "0");
        key(&mut app, KeyCode::PageDown);
        assert_eq!(top(&mut terminal, &mut app), "5");
        key(&mut app, KeyCode::PageDown);
        assert_eq!(top(&mut terminal, &mut app), "10");
        key(&mut app, KeyCode::PageUp);
        assert_eq!(top(&mut terminal, &mut app), "5");
        // 挿入モードでも効く。
        press(&mut app, "i");
        key(&mut app, KeyCode::PageDown);
        assert_eq!(top(&mut terminal, &mut app), "10");
    }

    /// :set number。
    #[test]
    fn line_numbers() {
        let mut terminal = Terminal::new(
            ratatui::backend::TestBackend::new(30, 8),
        )
        .unwrap();
        let mut app = App::new();
        press(&mut app, "if\n\ta\nb\x1b");
        assert_eq!(app.to_scheme(), "(f a b)");
        // 既定では出ない。
        assert_eq!(
            screen(&mut terminal, &mut app)[0],
            "f"
        );
        // ノード番号は1から。5Gの飛び先と一致する。
        press(&mut app, ":set number\n");
        assert!(app.number);
        assert_eq!(
            screen(&mut terminal, &mut app),
            vec![
                "  1 f",
                "  2 ├── a",
                "  3 └── b",
            ]
        );
        // 略記とトグル。
        press(&mut app, ":set nonu\n");
        assert!(!app.number);
        press(&mut app, ":set nu!\n");
        assert!(app.number);
        press(&mut app, ":set nu!\n");
        assert!(!app.number);
        press(&mut app, ":set nu\n");
        assert!(app.number);
        // 複数まとめて指定できる。
        press(&mut app, ":set nonumber number\n");
        assert!(app.number);
        // 未知の項目。
        press(&mut app, ":set zzz\n");
        assert_eq!(
            app.message,
            "不明な設定項目です: zzz"
        );
        press(&mut app, ":set\n");
        assert_eq!(app.message, "設定項目がありません");
        // ソース表示は出力の行番号。
        app.toggle_source_view();
        assert_eq!(
            screen(&mut terminal, &mut app),
            vec!["  1 (f a b)"]
        );
    }

    /// 桁幅はノード数が増えてもすぐには変わらない。
    #[test]
    fn number_width_is_stable() {
        // 3桁までは同じ幅。
        assert_eq!(number_width(1), 4);
        assert_eq!(number_width(9), 4);
        assert_eq!(number_width(10), 4);
        assert_eq!(number_width(999), 4);
        // 超えたら広げる。
        assert_eq!(number_width(1000), 5);
    }

    /// :42 で行へ移動する。
    #[test]
    fn goto_line() {
        let mut terminal = Terminal::new(
            ratatui::backend::TestBackend::new(20, 10),
        )
        .unwrap();
        let mut app = App::new();
        press(&mut app, "i0");
        for n in 1..20 {
            press(&mut app, &format!("\n{}", n));
        }
        press(&mut app, "\x1b");
        // 1から数える。番号はノードの通し番号。
        press(&mut app, ":1\n");
        assert_eq!(app.cursor, 0);
        press(&mut app, ":12\n");
        assert_eq!(app.cursor, 11);
        // 画面も付いてくる。
        press(&mut app, ":set number\n");
        assert_eq!(
            screen(&mut terminal, &mut app)[6],
            " 12 11"
        );
        // :0 は先頭。
        press(&mut app, ":0\n");
        assert_eq!(app.cursor, 0);
        // :$ は末尾。
        press(&mut app, ":$\n");
        assert_eq!(app.cursor, 19);
        // 行数を超えたら末尾で止まる。
        press(&mut app, ":1\n:999\n");
        assert_eq!(app.cursor, 19);
        // ソース表示でも同じくノードへ移動する。
        app.toggle_source_view();
        press(&mut app, ":10\n");
        assert_eq!(app.cursor, 9);
    }

    /// Y と D は部分木ごと。
    #[test]
    fn subtree_yank_cut() {
        // cond の節をまるごと複製する。
        let mut app = insert(
            "cond\n\t\n\t<\n\tx\n2\n\x081\n\x08\n\telse\n0",
        );
        press(&mut app, "\x1b");
        assert_eq!(
            app.to_scheme(),
            "(cond ((< x 2) 1)\n      (else 0))"
        );
        // 2行目の ◦ が節の頭。子孫は4つ。
        press(&mut app, ":2\nY");
        assert_eq!(
            app.message,
            "5ノードをヤンクしました"
        );
        // 次の節の頭に移り、手前に入れる。
        press(&mut app, ":7\nP");
        assert_eq!(
            app.to_scheme(),
            "(cond ((< x 2) 1)\n      ((< x 2) 1)\n      (else 0))"
        );
        // D は同じ範囲を切り取る。
        press(&mut app, "D");
        assert_eq!(
            app.to_scheme(),
            "(cond ((< x 2) 1)\n      (else 0))"
        );
        // 切り取ったものは貼り戻せる。
        press(&mut app, "P");
        assert_eq!(
            app.to_scheme(),
            "(cond ((< x 2) 1)\n      ((< x 2) 1)\n      (else 0))"
        );
        // 回数は受けない。
        let mut app = insert("f\n\ta\nb\nc");
        press(&mut app, "\x1bgg");
        press(&mut app, "j3Y");
        assert_eq!(
            app.message,
            "1ノードをヤンクしました"
        );
        // 葉の上では1ノードだけ。dd と同じ結果になる。
        press(&mut app, "D");
        assert_eq!(app.to_scheme(), "(f b c)");
        // 根の上なら全部消える。
        let mut app = insert("f\n\ta\nb");
        press(&mut app, "\x1bggD");
        assert_eq!(app.to_scheme(), "()");
        press(&mut app, "u");
        assert_eq!(app.to_scheme(), "(f a b)");
    }


    /// Normalモードの >> << Tab Shift-Tab は
    /// 部分木ごと動かす。子孫を置き去りにしない。
    #[test]
    fn subtree_indent() {
        // (f (a b)) で a を << すると、以前は
        // 深さが 0 から 2 に飛んでbが消えていた。
        let mut app = insert("f\n\ta\n\tb");
        press(&mut app, "\x1b");
        assert_eq!(app.to_scheme(), "(f (a b))");
        press(&mut app, "k<<");
        assert_eq!(app.to_scheme(), "f\n\n(a b)");
        assert_eq!(
            app.nodes.iter().map(|n| n.depth).collect::<Vec<_>>(),
            vec![0, 0, 1]
        );
        // f a (b) で a を >> すると、以前はbが
        // aの子から兄弟に変わっていた。
        let mut app = insert("f\na\n\tb");
        press(&mut app, "\x1b");
        assert_eq!(app.to_scheme(), "f\n\n(a b)");
        press(&mut app, "k>>");
        assert_eq!(app.to_scheme(), "(f (a b))");
        assert_eq!(
            app.nodes.iter().map(|n| n.depth).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        // Tab/Shift-Tabも同じ。
        press(&mut app, "\x08");
        assert_eq!(app.to_scheme(), "f\n\n(a b)");
        press(&mut app, "\t");
        assert_eq!(app.to_scheme(), "(f (a b))");
        // 上限は従来のindentと同じ。
        let mut app = insert("f\n\ta");
        press(&mut app, "\x1bk>>");
        assert_eq!(app.to_scheme(), "(f a)");
        // 深さ0では<<は何もしない。
        let mut app = insert("a\nb");
        press(&mut app, "\x1bgg<<");
        assert_eq!(app.to_scheme(), "a\n\nb");
    }

    /// Insertモードの Tab/Shift-Tab は従来通り1行だけ。
    ///
    /// aだけ動かしてbは置き去りにする。bの深さは
    /// 変わらないままaとの間に穴が空くので、
    /// to_scheme()には出なくなる（既知の性質）。
    #[test]
    fn insert_mode_indent_is_single_line() {
        let mut app = insert("f\n\ta\n\tb");
        press(&mut app, "\x1b");
        assert_eq!(app.to_scheme(), "(f (a b))");
        press(&mut app, "ki\x08");
        assert_eq!(
            app.nodes.iter().map(|n| n.depth).collect::<Vec<_>>(),
            vec![0, 0, 2]
        );
    }

    /// Enterはカーソル位置でテキストを分ける。
    #[test]
    fn enter_splits_text() {
        // abcの真ん中でEnter。
        let mut app = insert("abc");
        press(&mut app, "\x1bhi\n");
        assert_eq!(app.to_scheme(), "ab\n\nc");
        assert_eq!(app.cursor, 1);
        assert_eq!(app.cursor_col, 0);
        // 末尾でEnterすれば次は空のまま。
        let mut app = insert("abc");
        press(&mut app, "\n");
        assert_eq!(app.to_scheme(), "abc\n\n()");
        // 先頭でEnterすれば元のノードが空になる。
        let mut app = insert("abc");
        press(&mut app, "\x1b0i\n");
        assert_eq!(app.to_scheme(), "()\n\nabc");
    }

    /// Insertモードの End は行末へ。
    #[test]
    fn end_key_moves_to_end() {
        let mut app = insert("abc");
        press(&mut app, "\x1b0i");
        assert_eq!(app.cursor_col, 0);
        app.handle_key(KeyEvent::new(
            KeyCode::End,
            KeyModifiers::NONE,
        ));
        assert_eq!(app.cursor_col, 3);
    }

    /// 行頭のBackspaceは前のノードに合流する。
    /// enter_splits_textの逆になる。
    #[test]
    fn backspace_joins_with_previous() {
        // ab / c を合流すると abc に戻る。
        let mut app = insert("ab\nc");
        assert_eq!(app.to_scheme(), "ab\n\nc");
        press(&mut app, "\x1b0i");
        press(&mut app, "\x7f");
        assert_eq!(app.to_scheme(), "abc");
        assert_eq!(app.cursor, 0);
        assert_eq!(app.cursor_col, 2);
        // 消したノードに子があれば持ち上がる。
        let mut app = insert("f\n\ta\n\x08b\n\tc");
        press(&mut app, "\x1b");
        assert_eq!(app.to_scheme(), "(f a)\n\n(b c)");
        // b (3番目のノード) の先頭でBackspace。
        // bはaの末尾に合流し、子だったcは
        // 一段浅いabの兄弟としてfの子に入る。
        press(&mut app, ":3\n0i");
        press(&mut app, "\x7f");
        assert_eq!(app.to_scheme(), "(f ab c)");
        // 先頭のノードでは何もしない。
        let mut app = insert("a");
        press(&mut app, "\x1b0i\x7f");
        assert_eq!(app.to_scheme(), "a");
    }

    /// Insertモードの Home は行頭へ。
    #[test]
    fn home_key_moves_to_start() {
        let mut app = insert("abc");
        assert_eq!(app.cursor_col, 3);
        app.handle_key(KeyEvent::new(
            KeyCode::Home,
            KeyModifiers::NONE,
        ));
        assert_eq!(app.cursor_col, 0);
    }

    /// 行末のDeleteは次のノードと合流する。
    /// backspace_joins_with_previousの逆方向。
    #[test]
    fn delete_joins_with_next() {
        // ab / c を合流すると abc に戻る。
        let mut app = insert("ab\nc");
        assert_eq!(app.to_scheme(), "ab\n\nc");
        press(&mut app, "\x1bggi");
        app.handle_key(KeyEvent::new(
            KeyCode::End,
            KeyModifiers::NONE,
        ));
        app.handle_key(KeyEvent::new(
            KeyCode::Delete,
            KeyModifiers::NONE,
        ));
        assert_eq!(app.to_scheme(), "abc");
        assert_eq!(app.cursor, 0);
        assert_eq!(app.cursor_col, 2);
        // 引き込んだノードに子があれば持ち上がる。
        let mut app = insert("f\n\ta\n\x08b\n\tc");
        press(&mut app, "\x1b");
        assert_eq!(app.to_scheme(), "(f a)\n\n(b c)");
        // aの行末でDelete。bを引き込み、
        // bの子だったcはabの兄弟としてfの子に入る。
        press(&mut app, ":2\ni");
        app.handle_key(KeyEvent::new(
            KeyCode::End,
            KeyModifiers::NONE,
        ));
        app.handle_key(KeyEvent::new(
            KeyCode::Delete,
            KeyModifiers::NONE,
        ));
        assert_eq!(app.to_scheme(), "(f ab c)");
        // 最後のノードでは何もしない。
        let mut app = insert("a");
        app.handle_key(KeyEvent::new(
            KeyCode::Delete,
            KeyModifiers::NONE,
        ));
        assert_eq!(app.to_scheme(), "a");
    }

    /// NormalモードのDeleteはxと同じ。回数も効く。
    #[test]
    fn delete_key_in_normal_mode() {
        let mut app = insert("abc");
        press(&mut app, "\x1b0");
        app.handle_key(KeyEvent::new(
            KeyCode::Delete,
            KeyModifiers::NONE,
        ));
        assert_eq!(app.to_scheme(), "bc");
        press(&mut app, "2");
        app.handle_key(KeyEvent::new(
            KeyCode::Delete,
            KeyModifiers::NONE,
        ));
        assert_eq!(app.to_scheme(), "()");
        // ◦ の上ではノードごと消す。
        let mut app = insert("f\n\ta\n");
        press(&mut app, "\x1b");
        assert_eq!(app.to_scheme(), "(f a ())");
        app.handle_key(KeyEvent::new(
            KeyCode::Delete,
            KeyModifiers::NONE,
        ));
        assert_eq!(app.to_scheme(), "(f a)");
    }

    /// EscとすぐあとのキーがAlt+キーに合流しても、
    /// Escに続けてそのキーを打ったのと同じになる。
    #[test]
    fn alt_key_splits_into_esc_and_key() {
        // Esc+: でNormalに抜けてコマンド入力に入る。
        let mut app = insert("abc");
        app.handle_key(KeyEvent::new(
            KeyCode::Char(':'),
            KeyModifiers::ALT,
        ));
        assert_eq!(app.mode, Mode::Command);
        assert_eq!(app.to_scheme(), "abc");
        // Esc+i でNormalに抜けてすぐInsertへ戻る。
        // 実際に打った1文字だけが入る。
        let mut app = insert("abc");
        press(&mut app, "\x1b0");
        app.handle_key(KeyEvent::new(
            KeyCode::Char('i'),
            KeyModifiers::ALT,
        ));
        assert_eq!(app.mode, Mode::Insert);
        press(&mut app, "X");
        assert_eq!(app.to_scheme(), "Xabc");
        // Normalモードでも同じキーとして届く。
        // Esc+dd でノードを1つ消す。
        let mut app = insert("f\n\ta\nb");
        press(&mut app, "\x1b");
        assert_eq!(app.to_scheme(), "(f a b)");
        app.handle_key(KeyEvent::new(
            KeyCode::Char('d'),
            KeyModifiers::ALT,
        ));
        press(&mut app, "d");
        assert_eq!(app.to_scheme(), "(f a)");
    }

    /// 木の表示は折り返さず、右端を超えた行は
    /// 見切れた印を出しながら横スクロールする。
    #[test]
    fn horizontal_scroll() {
        let mut terminal = Terminal::new(
            ratatui::backend::TestBackend::new(15, 6),
        )
        .unwrap();
        let mut app = insert(
            "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcd",
        );
        press(&mut app, "\x1b0");
        // widthはdraw()を通すまで分からない。
        // 幅13（枠2を引いた分）に収まる範囲だけ見え、
        // 右に続きがあることを › で示す。
        assert_eq!(
            screen(&mut terminal, &mut app)[0],
            "0123456789AB›"
        );
        assert_eq!(app.width, 13);
        assert_eq!(app.h_scroll, 0);
        // 行末までカーソルを進めると左が見切れ、
        // ‹ が出る。カーソル（行末の仮想空白）は
        // 印の裏に隠れず必ず見える範囲に入る。
        // h_scrollはdraw()の中で決まるので、
        // 確認の前に必ずscreen()で描画する。
        press(&mut app, "$");
        let line = screen(&mut terminal, &mut app)[0].clone();
        assert_eq!(app.h_scroll, 30);
        assert!(line.starts_with('‹'));
        assert!(line.ends_with('d'));
        // 中ほどに戻ると両端に印が出て、カーソルの
        // 文字も欠けずに見える。
        press(&mut app, "020l");
        assert_eq!(
            screen(&mut terminal, &mut app)[0],
            "‹KLMNOPQRSTU›"
        );
        assert_eq!(app.h_scroll, 20);
        // 短い行では画面に収まるので印は出ない。
        let mut app = insert("f\n\ta");
        press(&mut app, "\x1b");
        assert_eq!(
            screen(&mut terminal, &mut app)
                .into_iter()
                .take(2)
                .collect::<Vec<_>>(),
            vec!["f", "└── a"]
        );
    }

    /// ソース表示（F2）は折り返す。カーソル行という
    /// 概念が無いので横スクロールは持ち込まない。
    #[test]
    fn source_view_wraps() {
        // 折り返した全行が収まるだけの高さを取る。
        let mut terminal = Terminal::new(
            ratatui::backend::TestBackend::new(15, 12),
        )
        .unwrap();
        let mut app = insert(
            "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcd",
        );
        press(&mut app, "\x1b");
        app.toggle_source_view();
        let lines = screen(&mut terminal, &mut app);
        // 折り返されて複数行になり、全文字が残る。
        assert!(lines.len() > 1);
        assert_eq!(
            lines.concat(),
            "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcd"
        );
    }

    /// NormalモードのHome/Endは0と$と同じ。
    #[test]
    fn home_end_in_normal_mode() {
        let mut app = insert("abc");
        press(&mut app, "\x1b");
        app.handle_key(KeyEvent::new(
            KeyCode::Home,
            KeyModifiers::NONE,
        ));
        assert_eq!(app.cursor_col, 0);
        app.handle_key(KeyEvent::new(
            KeyCode::End,
            KeyModifiers::NONE,
        ));
        assert_eq!(app.cursor_col, 3);
    }


    /// M は画面に見えている範囲の中央へ移動する。
    #[test]
    fn middle_of_screen() {
        let mut terminal = Terminal::new(
            ratatui::backend::TestBackend::new(20, 10),
        )
        .unwrap();
        let mut app = App::new();
        press(&mut app, "i0");
        for n in 1..20 {
            press(&mut app, &format!("\n{}", n));
        }
        press(&mut app, "\x1bgg");
        // 高さ7の画面で先頭から。中央はindex3。
        screen(&mut terminal, &mut app);
        assert_eq!(app.height, 7);
        press(&mut app, "M");
        assert_eq!(app.cursor, 3);
        // 1画面送ってから中央へ。
        press(&mut app, "\x1bgg");
        screen(&mut terminal, &mut app);
        press(&mut app, "\x06");
        screen(&mut terminal, &mut app);
        press(&mut app, "M");
        assert_eq!(app.cursor, app.scroll + app.height / 2);
        // draw()を通す前はheightが0なので、
        // 全体の中央へ寄せる。
        let mut app = insert("a\nb\nc");
        press(&mut app, "\x1bggM");
        assert_eq!(app.cursor, 1);
    }

    /// Backspace/Deleteは◦をノードごと消しても
    /// レジスタを汚さない。xは従来通りレジスタに入る。
    #[test]
    fn backspace_delete_do_not_yank() {
        // 先にyyでレジスタに何か入れておく。
        let mut app = insert("z\n\ta\n");
        press(&mut app, "\x1bggyy");
        assert_eq!(app.register.len(), 1);
        assert_eq!(app.to_scheme(), "(z a ())");
        // 末尾の ◦ を Delete で消す。
        press(&mut app, "G");
        app.handle_key(KeyEvent::new(
            KeyCode::Delete,
            KeyModifiers::NONE,
        ));
        assert_eq!(app.to_scheme(), "(z a)");
        // レジスタはyyのときのまま変わらない。
        press(&mut app, "p");
        assert_eq!(app.to_scheme(), "(z a z)");
        // 挿入モードのBackspaceも同様。
        let mut app = insert("z\n\ta\n\t");
        press(&mut app, "\x1bggyy");
        assert_eq!(app.to_scheme(), "(z (a ()))");
        press(&mut app, "Gi\x7f");
        assert_eq!(app.to_scheme(), "(z a)");
        press(&mut app, "\x1bp");
        assert_eq!(app.to_scheme(), "(z a z)");
        // x は今まで通りレジスタに積む。
        let mut app = insert("z\n\ta\n");
        press(&mut app, "\x1bggyyGx");
        assert_eq!(app.to_scheme(), "(z a)");
        press(&mut app, "p");
        assert_eq!(app.to_scheme(), "(z a ())");
    }

    /// F2でソース表示を編集できる。
    ///
    /// 木をdepth:0の行に変換し、既存の編集エンジンを
    /// 使い回す。編集後にF2で戻すと読み直して木になる。
    #[test]
    fn edit_in_source_view() {
        let mut app = insert("f\n\ta\nb");
        press(&mut app, "\x1b");
        assert_eq!(app.to_scheme(), "(f a b)");

        app.toggle_source_view();
        assert!(app.source_mode);
        assert_eq!(
            app.nodes
                .iter()
                .map(|n| n.text.clone())
                .collect::<Vec<_>>(),
            vec!["(f a b)".to_string()]
        );
        assert_eq!(app.mode, Mode::Normal);

        // 行の途中に別の要素を書き足す。
        press(&mut app, "A c\x1b");
        assert_eq!(app.nodes[0].text, "(f a b) c");

        // F2で木に戻すと再パースされる。
        app.toggle_source_view();
        assert!(!app.source_mode);
        assert_eq!(app.to_scheme(), "(f a b)\n\nc");
    }

    /// 壊れた括弧のまま木には戻れない。
    #[test]
    fn source_view_parse_error_stays() {
        let mut app = insert("(a b)");
        press(&mut app, "\x1b");
        app.toggle_source_view();
        press(&mut app, "A (\x1b");
        app.toggle_source_view();
        assert!(app.source_mode);
        assert!(app.message.contains("木に戻せません"));
    }

    /// ソース表示の空行は◦にならない。
    #[test]
    fn source_view_blank_lines_stay_blank() {
        let mut app = insert("a");
        press(&mut app, "\x1b");
        press(&mut app, "obcd\x1b");
        assert_eq!(app.to_scheme(), "a\n\nbcd");
        app.toggle_source_view();
        assert_eq!(
            app.tree_lines(),
            vec!["a", "", "bcd"]
        );
    }

    /// ソース表示のTabは罫線ではなく空白を入れる。
    #[test]
    fn source_view_tab_inserts_spaces() {
        let mut app = insert("f");
        press(&mut app, "\x1b");
        app.toggle_source_view();
        press(&mut app, "A\tx\x1b");
        assert_eq!(app.nodes[0].text, "f  x");
        assert_eq!(app.nodes[0].depth, 0);
        assert_eq!(app.tree_lines(), vec!["f  x"]);
    }

    /// 木とソース表示でヤンクのレジスタは別。
    #[test]
    fn source_view_has_its_own_register() {
        let mut app = insert("f\n\ta\nb");
        press(&mut app, "\x1b");
        press(&mut app, "ggyy");
        assert_eq!(app.register.len(), 1);

        app.toggle_source_view();
        assert!(app.source_register.is_empty());

        press(&mut app, "ggyy");
        assert_eq!(app.source_register.len(), 1);
        assert!(app.register.len() == 1);

        // 貼り付けは今のモードのレジスタだけを見る。
        press(&mut app, "p");
        assert_eq!(
            app.nodes[1].text,
            app.nodes[0].text
        );
    }

    /// タイトルは常に編集モードを出す。ソース表示か
    /// どうかは中身を見れば分かるので出さない。
    #[test]
    fn title_shows_edit_mode_in_source_view() {
        let mut app = insert("f");
        press(&mut app, "\x1b");
        app.toggle_source_view();
        assert!(title(&app).contains("NORMAL"));
        assert!(!title(&app).contains("Scheme"));
        press(&mut app, "i");
        assert!(title(&app).contains("INSERT"));
    }

    /// undoは共有される。F2の切り替え自体は1段として
    /// 積まないが、Snapshotがsource_modeも一緒に控える
    /// ので、記録されている編集を遡って木の時点まで
    /// 戻ると表示モードも正しく木に戻る。
    #[test]
    fn undo_crosses_source_view_toggle() {
        // "a"の入力はtree_modeで記録される1段。
        let mut app = insert("f\n\ta");
        press(&mut app, "\x1b");
        assert_eq!(app.to_scheme(), "(f a)");

        app.toggle_source_view();
        assert!(app.source_mode);

        // ソース表示での追記もう1段。
        press(&mut app, "ob\x1b");
        assert_eq!(app.nodes.len(), 2);

        // 1回目のuはソース表示中の編集を戻すだけ。
        // 切り替え自体は記録していないので、
        // まだソース表示のまま。
        press(&mut app, "u");
        assert!(app.source_mode);
        assert_eq!(app.nodes.len(), 1);

        // さらに戻ると、木を作っていた時点の
        // スナップショットに達し、表示モードも
        // 一緒に木へ戻る。
        press(&mut app, "u");
        assert!(!app.source_mode);
    }

    /// F2で切り替えたとき、カーソルの対応する位置に
    /// 着地する（毎回先頭に戻らない）。
    #[test]
    fn f2_preserves_cursor_position() {
        let mut app = insert(
            "define\n\tf\nx\n\x08define\n\tg\ny",
        );
        press(&mut app, "\x1b");
        assert_eq!(
            app.to_scheme(),
            "(define f\n  x)\n\n(define g\n  y)"
        );

        // 2つ目のdefineの見出し（g）にカーソルを置く。
        let g = app
            .nodes
            .iter()
            .position(|n| n.text == "g")
            .unwrap();
        app.cursor = g;

        app.toggle_source_view();
        assert!(app.source_mode);
        // gの行に着地する。fの行には戻らない。
        assert_eq!(app.nodes[app.cursor].text, "(define g");

        // 木に戻すと、同じ行にある g のノードに戻る。
        app.toggle_source_view();
        assert!(!app.source_mode);
        assert_eq!(app.nodes[app.cursor].text, "g");
    }

    /// 1行に収まる子ノードにカーソルがあっても、
    /// 畳んだ親の位置に着地する。
    #[test]
    fn f2_falls_back_to_enclosing_node() {
        let mut app = insert("+\n\t1\n2");
        press(&mut app, "\x1b");
        assert_eq!(app.to_scheme(), "(+ 1 2)");

        // "2" は "+" と同じ1行に畳まれているので、
        // 個別の記録を持たない。
        press(&mut app, "j");
        assert_eq!(app.text(), "2");

        app.toggle_source_view();
        assert_eq!(app.cursor, 0);
        assert_eq!(app.nodes[0].text, "(+ 1 2)");
    }


    /// 列も、対応するノードのテキストが始まる
    /// 位置に合わせる。
    #[test]
    fn f2_preserves_cursor_column() {
        let mut app = insert("abcdef");
        press(&mut app, "\x1b0lll"); // 列3(d)へ。
        assert_eq!(app.cursor_col, 3);

        app.toggle_source_view();
        assert_eq!(app.cursor_col, 3);
        assert_eq!(app.nodes[0].text, "abcdef");
    }

    /// 子を持つノード自身にカーソルがあるときも、
    /// 開き括弧の分ずれずに列が合う。
    #[test]
    fn f2_preserves_cursor_column_with_children() {
        let mut app = insert("f\n\ta");
        press(&mut app, "\x1bgg0"); // fノード、列0。
        assert_eq!(app.to_scheme(), "(f a)");
        app.toggle_source_view();
        // "(f a)" の f は列1（開き括弧の次）。
        assert_eq!(app.nodes[0].text, "(f a)");
        assert_eq!(app.cursor_col, 1);
    }





    /// crosstermは生モードで \n を素のEnterではなく
    /// Ctrl+jとして届ける。貼り付けた複数行のテキストが
    /// 1文字ずつ届くとき、改行のたびにこれが起きる。
    #[test]
    fn ctrl_j_is_enter() {
        // 貼り付けを模して、Ctrl+j混じりで1文字ずつ流す。
        let mut app = insert("ab");
        app.handle_key(KeyEvent::new(
            KeyCode::Char('j'),
            KeyModifiers::CONTROL,
        ));
        press(&mut app, "cd");
        assert_eq!(app.to_scheme(), "ab\n\ncd");
        // jという文字が紛れ込んでいないこと。
        assert_eq!(app.nodes[0].text, "ab");
        assert_eq!(app.nodes[1].text, "cd");

        // Normalモードでも同じキーとして届く。
        let mut app = insert("x");
        press(&mut app, "\x1b");
        app.handle_key(KeyEvent::new(
            KeyCode::Char('j'),
            KeyModifiers::CONTROL,
        ));
        // NormalモードのEnterは何もしない
        // （挿入モードのように行を分けたりしない）。
        assert_eq!(app.to_scheme(), "x");
    }

    /// 複数行の貼り付けを1文字ずつ模して、実際の
    /// 報告に近い状況（ソース表示での貼り付け）を
    /// 確かめる。改行のたびにCtrl+jを挟む。
    #[test]
    fn paste_multiline_into_source_view() {
        let mut app = insert("f\n\ta");
        press(&mut app, "\x1b");
        assert_eq!(app.to_scheme(), "(f a)");

        app.toggle_source_view();
        press(&mut app, "A");

        let words = ["new", "line"];
        for (i, word) in words.iter().enumerate() {
            if i > 0 {
                app.handle_key(KeyEvent::new(
                    KeyCode::Char('j'),
                    KeyModifiers::CONTROL,
                ));
            }
            press(&mut app, word);
        }

        press(&mut app, "\x1b");
        app.toggle_source_view();
        // "(f a)" の直後に "new" と "line" が独立した
        // 式として増える。jという文字が紛れ込んでいない
        // こと。
        assert_eq!(
            app.to_scheme(),
            "(f a)\n\nnew\n\nline"
        );
    }

    /// bracketed pasteは、中身がキー入力として
    /// 再解釈されず、テキストとしてそのまま入る。
    #[test]
    fn bracketed_paste_in_insert_mode() {
        let mut app = insert("f\n\ta");
        press(&mut app, "\x1b");
        assert_eq!(app.to_scheme(), "(f a)");

        // 貼り付けの中身にTabが混じっていても、
        // ノードのテキストにそのまま入る（キー入力の
        // Tabのように空白2つへ変換されたりしない）。
        // ソース表示のまま確認する。木に戻すと
        // Schemeのリーダーがタブを区切りとして読み、
        // 別トークンに分かれてしまうため。
        app.toggle_source_view();
        press(&mut app, "A");
        app.handle_paste("new\tline\nsecond");

        assert_eq!(app.nodes[0].text, "(f a)new\tline");
        assert_eq!(app.nodes[1].text, "second");
    }

    /// Normalモードでの貼り付けは、内容がコマンド列
    /// として実行されず、1回の編集としてテキストが
    /// 挿入される。
    #[test]
    fn bracketed_paste_in_normal_mode() {
        let mut app = insert("f\n\ta\nb");
        press(&mut app, "\x1b");
        assert_eq!(app.to_scheme(), "(f a b)");

        // ddを含む文字列を貼り付けても、ノードを
        // 消したりしない。
        press(&mut app, "gg0");
        app.handle_paste("ddyyx");
        assert_eq!(app.nodes[0].text, "ddyyxf");
        assert_eq!(app.to_scheme(), "(ddyyxf a b)");

        // 1回のundoで全部戻る。
        press(&mut app, "u");
        assert_eq!(app.to_scheme(), "(f a b)");
    }

    /// コマンド行への貼り付けは最初の行だけ入る。
    #[test]
    fn bracketed_paste_in_command_mode() {
        let mut app = insert("f");
        press(&mut app, "\x1b:");
        app.handle_paste("w foo.scm\nrm -rf /");
        assert_eq!(app.command, "w foo.scm");
    }


}
