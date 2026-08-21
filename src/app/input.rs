use super::App;
use super::Mode;

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;

impl App {
    pub(crate) fn handle_key(
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

        // 検索パターンの入力中も同様。
        if self.mode == Mode::Search {
            self.handle_search(key.code);
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
            Mode::Search => {}
        }

        !self.quit
    }

    pub(crate) fn handle_command(&mut self, code: KeyCode) {
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

    pub(crate) fn handle_search(&mut self, code: KeyCode) {
        match code {
            KeyCode::Esc => {
                self.search.clear();
                self.mode = Mode::Normal;
            }
            KeyCode::Enter => {
                let pattern =
                    std::mem::take(&mut self.search);
                self.mode = Mode::Normal;
                self.search_and_move(
                    &pattern,
                    self.search_forward,
                );
            }
            KeyCode::Backspace => {
                // 空の状態で消すと抜ける。
                if self.search.pop().is_none() {
                    self.mode = Mode::Normal;
                }
            }
            KeyCode::Char(c) => self.search.push(c),
            _ => {}
        }
    }

    /// モードによらない操作。処理したらtrueを返す。
    pub(crate) fn handle_common(&mut self, code: KeyCode) -> bool {
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

    pub(crate) fn handle_normal(&mut self, key: KeyEvent) {
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
            '/' => {
                self.search.clear();
                self.message.clear();
                self.search_forward = true;
                self.mode = Mode::Search;
            }
            '?' => {
                self.search.clear();
                self.message.clear();
                self.search_forward = false;
                self.mode = Mode::Search;
            }
            // 直前の検索を同じ方向・逆方向に繰り返す。
            'n' => self.repeat_search(true),
            'N' => self.repeat_search(false),
            // カーソルのノードのテキストをそのまま
            // パターンにして前方・後方検索する。
            '*' => self.search_word(true),
            '#' => self.search_word(false),
            _ => {}
        }
    }

    /// bracketed pasteで届いたテキストを反映する。
    ///
    /// コマンド行は1行しか持てないので最初の行だけ。
    /// Normalモードでの貼り付けは、そのままだと
    /// 文字列がキー入力として解釈されコマンド列に
    /// なってしまう危険があるため、1つの編集として
    /// 直接テキストを挿入する（vimと同じ扱い）。
    pub(crate) fn handle_paste(&mut self, text: &str) {
        match self.mode {
            Mode::Command => {
                if let Some(first) = text.lines().next() {
                    self.command.push_str(first);
                }
            }
            Mode::Search => {
                if let Some(first) = text.lines().next() {
                    self.search.push_str(first);
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

    pub(crate) fn handle_insert(&mut self, code: KeyCode) {
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
