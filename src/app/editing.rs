use super::App;
use crate::node::normalize;
use crate::node::Node;

impl App {
    /// カーソルの1つ上に同じ深さの空ノードを作る。
    pub(crate) fn open_above(&mut self) {
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
    pub(crate) fn delete_char(&mut self, yank: bool) {
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

    pub(crate) fn insert_char(&mut self, c: char) {
        let node = &mut self.nodes[self.cursor];

        node.text.insert(self.cursor_col, c);
        self.cursor_col += c.len_utf8();
        self.record();
    }

    /// 貼り付けたテキストをカーソル位置に反映する。
    /// 改行のたびにEnterと同じくノードを分ける。
    pub(crate) fn paste_text(&mut self, text: &str) {
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

    /// カーソルの手前の1文字を消す。
    ///
    /// ◦ の上ではノードごと消して、前のノードの
    /// 末尾に戻る。Enterを取り消す動きになる。
    /// レジスタは汚さない。
    pub(crate) fn backspace(&mut self) {
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
    pub(crate) fn enter(&mut self) {
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
    pub(crate) fn indent(&mut self) {
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

    pub(crate) fn unindent(&mut self) {
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
    pub(crate) fn indent_subtree(&mut self) {
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
    pub(crate) fn unindent_subtree(&mut self) {
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

    pub(crate) fn move_up(&mut self) {
        if self.cursor == 0 {
            return;
        }

        self.cursor -= 1;

        self.cursor_col = self.cursor_col.min(
            self.nodes[self.cursor].text.len()
        );
    }

    pub(crate) fn move_down(&mut self) {
        if self.cursor + 1 >= self.nodes.len() {
            return;
        }

        self.cursor += 1;

        self.cursor_col = self.cursor_col.min(
            self.nodes[self.cursor].text.len()
        );
    }

    pub(crate) fn move_left(&mut self) {
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

    pub(crate) fn move_right(&mut self) {
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
    pub(crate) fn repair(&mut self) {
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
    pub(crate) fn delete_node(&mut self, count: usize, yank: bool) {
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
    pub(crate) fn subtree_end(&self, index: usize) -> usize {
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
    pub(crate) fn subtree_len(&self) -> usize {
        self.subtree_end(self.cursor) - self.cursor
    }

    /// カーソルから count 個のノードをヤンクする。
    pub(crate) fn yank(&mut self, count: usize) {
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
    pub(crate) fn paste(&mut self, before: bool, count: usize) {
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
}
