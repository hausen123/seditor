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

            lines.push(format!(
                "{}{}{}",
                prefix,
                self.nodes[index].text,
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

    fn find_ancestor(
        &self,
        index: usize,
        level: usize,
    ) -> usize {
        let target_depth =
            self.nodes[index].depth - level - 1;

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

    fn to_scheme(&self) -> String {
        if self.nodes.is_empty() {
            return String::new();
        }

        let mut output = String::new();

        self.write_expression(
            0,
            self.nodes[0].depth,
            &mut output,
        );

        output
    }

    /// indexを先頭とする1つのS式を書き出す。
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
    /// (define (square x) (* x x))
    ///
    /// にする。
    fn write_expression(
        &self,
        index: usize,
        depth: usize,
        output: &mut String,
    ) -> usize {
        output.push('(');

        output.push_str(
            self.nodes[index].text.trim()
        );

        let mut i = index + 1;

        while i < self.nodes.len() {
            let child_depth =
                self.nodes[i].depth;

            if child_depth <= depth {
                break;
            }

            if child_depth == depth + 1 {
                output.push(' ');

                i = self.write_expression(
                    i,
                    child_depth,
                    output,
                );
            } else {
                // 不正な深さになっている場合。
                //
                // 通常の操作では発生しないように
                // indent()で制限している。
                i += 1;
            }
        }

        output.push(')');

        i
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
