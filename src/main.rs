use std::io;
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
enum Style {
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

struct App {
    nodes: Vec<Node>,
    cursor: usize,
    cursor_col: usize,
    source_mode: bool,
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
        }
    }

    fn insert_char(&mut self, c: char) {
        let node = &mut self.nodes[self.cursor];

        node.text.insert(self.cursor_col, c);
        self.cursor_col += c.len_utf8();
    }

    fn backspace(&mut self) {
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
        }
    }

    fn unindent(&mut self) {
        if self.nodes[self.cursor].depth > 0 {
            self.nodes[self.cursor].depth -= 1;
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

    fn delete_node(&mut self) {
        if self.nodes.len() == 1 {
            self.nodes[0].text.clear();
            self.nodes[0].depth = 0;
            self.cursor_col = 0;
            return;
        }

        self.nodes.remove(self.cursor);

        if self.cursor >= self.nodes.len() {
            self.cursor = self.nodes.len() - 1;
        }

        self.cursor_col = self.cursor_col.min(
            self.nodes[self.cursor].text.len()
        );
    }

    // ------------------------------------------------------------
    // Tree表示
    // ------------------------------------------------------------

    fn tree_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();

        for index in 0..self.nodes.len() {
            let prefix = self.tree_prefix(index);

            let cursor = if index == self.cursor {
                "▌"
            } else {
                ""
            };

            let text = if self.nodes[index].text.is_empty() {
                "◦"
            } else {
                &self.nodes[index].text
            };

            lines.push(format!(
                "{}{}{}",
                prefix,
                text,
                cursor
            ));
        }

        lines
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

    fn style(&self, index: usize) -> Style {
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
            return Style::Body(2);
        }
        match text {
            "define" | "lambda" | "let" | "let*"
            | "letrec" | "letrec*" | "when"
            | "unless" | "case" => Style::Body(1),
            "do" => Style::Body(2),
            "begin" => Style::Body(0),
            _ => Style::Align,
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
        match self.style(index) {
            Style::Body(count) => {
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
            Style::Align => {
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

        match key.code {
            KeyCode::F(2) => {
                self.source_mode =
                    !self.source_mode;
            }

            KeyCode::Tab => {
                self.indent();
            }

            KeyCode::BackTab => {
                self.unindent();
            }

            KeyCode::Enter => {
                self.enter();
            }

            KeyCode::Backspace => {
                self.backspace();
            }

            KeyCode::Delete => {
                self.delete_node();
            }

            KeyCode::Up => {
                self.move_up();
            }

            KeyCode::Down => {
                self.move_down();
            }

            KeyCode::Left => {
                self.move_left();
            }

            KeyCode::Right => {
                self.move_right();
            }

            KeyCode::Char(c) => {
                self.insert_char(c);
            }

            _ => {}
        }

        true
    }
}

// ------------------------------------------------------------
// 描画
// ------------------------------------------------------------

fn draw(
    frame: &mut Frame,
    app: &App,
) {
    let area = frame.area();

    let text = if app.source_mode {
        app.to_scheme()
    } else {
        app.tree_lines().join("\n")
    };

    let title = if app.source_mode {
        " Gauche Tree - Scheme "
    } else {
        " Gauche Tree "
    };

    let paragraph = Paragraph::new(text)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL),
        );

    frame.render_widget(
        paragraph,
        area,
    );
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

    let result =
        run(&mut terminal);

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
) -> io::Result<()> {
    let mut app = App::new();

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
    /// `\t` はTab、`\n` はEnter、`\x08` はShift-Tab。
    fn press(app: &mut App, keys: &str) {
        for key in keys.chars() {
            let code = match key {
                '\t' => KeyCode::Tab,
                '\n' => KeyCode::Enter,
                '\x08' => KeyCode::BackTab,
                _ => KeyCode::Char(key),
            };
            app.handle_key(KeyEvent::new(
                code,
                KeyModifiers::NONE,
            ));
        }
    }

    /// キー列を打ってからF2の出力を得る。
    fn scheme(keys: &str) -> String {
        let mut app = App::new();
        press(&mut app, keys);
        app.to_scheme()
    }

    /// キー列を打ってからカーソル表示を除いた木を得る。
    fn tree(keys: &str) -> String {
        let mut app = App::new();
        press(&mut app, keys);
        app.tree_lines().join("\n").replace('▌', "")
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
}
