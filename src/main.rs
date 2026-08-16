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
        Event,
        KeyCode,
        KeyEvent,
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
            nodes.push(Node {
                text: mark.clone(),
                depth,
            });
            emit(child, depth + 1, nodes);
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

/// 積んでおくアンドゥの段数。
const UNDO_LIMIT: usize = 100;

/// 巻き戻すために丸ごと控えておく状態。
#[derive(Clone)]
struct Snapshot {
    nodes: Vec<Node>,
    cursor: usize,
    cursor_col: usize,
}

#[derive(PartialEq)]
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
            held: None,
        }
    }

    // ------------------------------------------------------------
    // アンドゥ
    // ------------------------------------------------------------

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            nodes: self.nodes.clone(),
            cursor: self.cursor,
            cursor_col: self.cursor_col,
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

        let mut text = self.to_scheme();
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
    fn delete_char(&mut self) {
        if self.text().is_empty() {
            self.delete_node();
            return;
        }

        let column = self.cursor_col;
        let node = &mut self.nodes[self.cursor];
        if column >= node.text.len() {
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

    /// カーソルの手前の1文字を消す。
    ///
    /// ◦ の上ではノードごと消して、前のノードの
    /// 末尾に戻る。Enterを取り消す動きになる。
    fn backspace(&mut self) {
        if self.text().is_empty() {
            if self.cursor == 0 {
                self.delete_node();
                self.cursor_col = 0;
                return;
            }

            // 消すと末尾のときにdelete_nodeが
            // カーソルを前に丸めるので、消す前の
            // 位置から前のノードを決めておく。
            let previous = self.cursor - 1;

            self.delete_node();

            self.cursor = previous;
            self.cursor_col = self.text().len();

            return;
        }

        if self.cursor_col == 0 {
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

    fn enter(&mut self) {
        let depth = self.nodes[self.cursor].depth;

        self.nodes.insert(
            self.cursor + 1,
            Node {
                text: String::new(),
                depth,
            },
        );

        self.cursor += 1;
        self.cursor_col = 0;
        self.record();
    }

    fn indent(&mut self) {
        if self.cursor == 0 {
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
        if self.nodes[self.cursor].depth > 0 {
            self.nodes[self.cursor].depth -= 1;
            self.record();
        }
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

    /// カーソルのノードを消し、その子孫を1段持ち上げる。
    ///
    /// 子を残したまま親だけ消すと深さが飛び、
    /// children()に拾われず出力から消えてしまう。
    fn delete_node(&mut self) {
        if self.nodes.len() == 1 {
            self.nodes[0].text.clear();
            self.nodes[0].depth = 0;
            self.cursor_col = 0;
            self.record();
            return;
        }
        let depth = self.nodes[self.cursor].depth;
        self.nodes.remove(self.cursor);
        let mut i = self.cursor;
        while i < self.nodes.len()
            && self.nodes[i].depth > depth
        {
            self.nodes[i].depth -= 1;
            i += 1;
        }
        if self.cursor >= self.nodes.len() {
            self.cursor = self.nodes.len() - 1;
        }
        self.cursor_col =
            self.cursor_col.min(self.text().len());
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

            let display = if text.is_empty() {
                "◦"
            } else {
                text
            };

            lines.push(format!("{}{}", prefix, display));
        }

        lines
    }

    /// 描画用の行。カーソルの1セルを反転させる。
    ///
    /// 記号を挿し込むと桁がずれるので、
    /// 文字数を変えずに見せる。
    fn tree_display(&self) -> Vec<Line<'static>> {
        let highlight = Style::default()
            .add_modifier(Modifier::REVERSED);

        self.tree_lines()
            .into_iter()
            .enumerate()
            .map(|(index, line)| {
                if index != self.cursor {
                    return Line::from(line);
                }
                // 空ノードは◦の上にカーソルを置く。
                let text = &self.nodes[index].text;
                let column = if text.is_empty() {
                    0
                } else {
                    self.cursor_col
                };
                let split =
                    self.tree_prefix(index).len() + column;
                let (left, rest) = line.split_at(split);
                let mut chars = rest.chars();
                // 行末より右にいるときは空白を反転させる。
                let (cell, right) = match chars.next() {
                    Some(c) => (
                        c.to_string(),
                        chars.as_str().to_string(),
                    ),
                    None => (" ".to_string(), String::new()),
                };
                Line::from(vec![
                    Span::raw(left.to_string()),
                    Span::styled(cell, highlight),
                    Span::raw(right),
                ])
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
        let mut output = String::new();
        for index in 0..self.nodes.len() {
            if self.nodes[index].depth != 0 {
                continue;
            }
            if !output.is_empty() {
                output.push_str("\n\n");
            }
            self.write_pretty(index, &mut output);
        }
        output
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

    /// '(a b) の ' のように、子1つの前に付いて
    /// 括弧を足さないノードかどうか。
    ///
    /// #t や #\a のように子を持たないものは
    /// この判定を通らずアトムとして扱われる。
    fn is_marker(&self, index: usize) -> bool {
        if self.children(index).len() != 1 {
            return false;
        }
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
            return format!(
                "{}{}",
                text,
                self.flat(children[0])
            );
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
    fn write_pretty(
        &self,
        index: usize,
        output: &mut String,
    ) {
        let indent = current_column(output);
        let text = self.nodes[index].text.trim();
        let children = self.children(index);
        if self.is_marker(index) {
            output.push_str(text);
            self.write_pretty(children[0], output);
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
                    self.write_pretty(child, output);
                }
                self.write_children(
                    &children[count..],
                    indent + 2,
                    output,
                );
            }
            Indent::Align => {
                if !text.is_empty() {
                    output.push(' ');
                }
                let column = current_column(output);
                self.write_pretty(children[0], output);
                self.write_children(
                    &children[1..],
                    column,
                    output,
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
    ) {
        for &child in children {
            output.push('\n');
            output.push_str(&" ".repeat(column));
            self.write_pretty(child, output);
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
        match code {
            KeyCode::F(2) => {
                self.source_mode = !self.source_mode
            }
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
            if key.code == KeyCode::Char('r') {
                self.redo();
            }
            return;
        }

        let KeyCode::Char(c) = key.code else {
            if key.code == KeyCode::Esc {
                self.pending = None;
            }
            return;
        };
        // dd や >> のような2打鍵の命令。
        if let Some(first) = self.pending.take() {
            match (first, c) {
                ('d', 'd') => {
                    self.begin_edit();
                    self.delete_node();
                }
                ('>', '>') => {
                    self.begin_edit();
                    self.indent();
                }
                ('<', '<') => {
                    self.begin_edit();
                    self.unindent();
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
                self.delete_char();
            }
            'u' => self.undo(),
            'h' => self.move_left(),
            'j' => self.move_down(),
            'k' => self.move_up(),
            'l' => self.move_right(),
            '0' => self.cursor_col = 0,
            '$' => self.cursor_col = self.text().len(),
            'G' => self.move_to(self.nodes.len() - 1),
            ':' => {
                self.command.clear();
                self.message.clear();
                self.mode = Mode::Command;
            }
            'd' | '>' | '<' | 'g' => {
                self.pending = Some(c)
            }
            _ => {}
        }
    }

    fn handle_insert(&mut self, code: KeyCode) {
        match code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Tab => self.indent(),
            KeyCode::BackTab => self.unindent(),
            KeyCode::Enter => self.enter(),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete_char(),
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
    app: &App,
) {
    // 下1行をコマンドとメッセージに使う。
    let areas = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(frame.area());

    let text: Text = if app.source_mode {
        app.to_scheme().into()
    } else {
        app.tree_display().into()
    };

    let paragraph = Paragraph::new(text)
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

    let mode = if app.source_mode {
        "Scheme"
    } else {
        match app.mode {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Command => "COMMAND",
        }
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
        EnterAlternateScreen
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
            draw(frame, &app);
        })?;

        if event::poll(
            Duration::from_millis(50)
        )? {
            if let Event::Key(key) =
                event::read()?
            {
                if !app.handle_key(key) {
                    break;
                }
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

    /// 印ノード。子1つの前に付き、括弧を足さない。
    #[test]
    fn markers() {
        check(&[
            ("'\n\ta\n\tb", "'(a b)"),
            ("`\n\ta\n\t,b", "`(a ,b)"),
            (",@\n\ta\n\tb", ",@(a b)"),
            ("#\n\t1\n\t2", "#(1 2)"),
            ("#u8\n\t1\n\t2", "#u8(1 2)"),
            ("#;\n\ta\n\tb", "#;(a b)"),
            ("#0=\n\ta\n\tb", "#0=(a b)"),
            // 入れ子の擬似引用
            ("`\n\ta\n\t`\n\tb\n\t,c", "`(a `(b ,c))"),
            // 引用付きシンボルはただのテキスト
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

    /// 印ノードの子が1つでない場合は普通のリストとして出す。
    #[test]
    fn marker_wrong_arity() {
        check(&[
            ("'\n\ta\nb", "(' a b)"),
            ("'", "'"),
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
        fn cell(app: &App) -> String {
            app.tree_display()[app.cursor].spans[1]
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
            "'(a b)",
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
}
