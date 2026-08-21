use super::App;
use crate::node::always_break;
use crate::node::current_column;
use crate::node::nodes_from;
use crate::node::number_span;
use crate::node::number_width;
use crate::node::Indent;
use crate::node::Node;
use crate::node::WIDTH;
use crate::reader;

use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;

impl App {
    /// F2。木とソース表示を切り替える。
    pub(super) fn toggle_source_view(&mut self) {
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
        self.center_on_cursor();
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
        self.center_on_cursor();
    }

    pub(super) fn tree_lines(&self) -> Vec<String> {
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
    pub(super) fn tree_display(&self) -> Vec<Line<'static>> {
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

    pub(super) fn tree_prefix(&self, index: usize) -> String {
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
    pub(super) fn to_scheme(&self) -> String {
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
    pub(super) fn children(&self, index: usize) -> Vec<usize> {
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
}
