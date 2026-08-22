use super::search::translate_vim_pattern;
use super::App;
use super::Mode;

use crossterm::event::KeyCode;

use regex::Captures;
use regex::Regex;
use regex::RegexBuilder;

/// 置換文字列を組み立てるためのトークン。
#[derive(Debug, Clone)]
enum ReplPart {
    Literal(String),
    Whole,
    Group(u8),
    UpperNext,
    LowerNext,
    UpperUntil,
    LowerUntil,
    EndCase,
}

/// g/i/I/n/e/c フラグの状態。
#[derive(Debug, Clone)]
struct SubFlags {
    global: bool,
    ignore_case: Option<bool>,
    no_replace: bool,
    quiet_empty: bool,
    confirm: bool,
}

impl Default for SubFlags {
    fn default() -> Self {
        Self {
            global: false,
            ignore_case: None,
            no_replace: false,
            quiet_empty: false,
            confirm: false,
        }
    }
}

/// `:&` `:&&` `g&` `~` のために覚えておく直前の置換。
#[derive(Clone)]
pub(super) struct LastSubstitute {
    pattern: String,
    replacement_raw: String,
    parts: Vec<ReplPart>,
    flags: SubFlags,
}

/// `c`フラグの対話確認の途中状態。
pub(super) struct ConfirmState {
    regex: Regex,
    parts: Vec<ReplPart>,
    global: bool,
    /// 走査中のノード番号。
    node: usize,
    /// 走査中ノード内で次に探し始める位置。
    pos: usize,
    /// 走査するノード番号の終端（含む）。
    range_end: usize,
    /// 直近に見つかったマッチの開始・終了位置。
    match_range: (usize, usize),
    /// ここまでに実際に置換した件数。
    count: usize,
    quiet_empty: bool,
    /// 一度でもConfirmモードでマッチを示せたか。
    ///
    /// 最初の探索から1件もマッチが無い場合だけ、
    /// 通常の「見つからない」メッセージにする。
    /// 一度でも確認を挟んだ後の終了は、count=0でも
    /// 「0箇所を置換しました」でよい。
    entered: bool,
}

/// `0-9 , . $ %` の集合を貪欲に読む。
fn range_prefix_len(s: &str) -> usize {
    s.chars()
        .take_while(|c| "0123456789,.$%".contains(*c))
        .map(|c| c.len_utf8())
        .sum()
}

/// エスケープされていない次の `/` の位置を探す。
///
/// `\`の次の1文字は無条件でスキップするので、`\/`は
/// 区切りとして扱われない。
fn find_unescaped_slash(s: &str) -> Option<usize> {
    let mut chars = s.char_indices();
    while let Some((i, c)) = chars.next() {
        if c == '\\' {
            chars.next();
        } else if c == '/' {
            return Some(i);
        }
    }
    None
}

/// `pattern/replacement/flags` を`/`で3つに割る。
/// 区切りが足りない分は空文字列にする。
fn split_substitute_body(body: &str) -> (&str, &str, &str) {
    match find_unescaped_slash(body) {
        Some(first) => {
            let pattern = &body[..first];
            let remainder = &body[first + 1..];
            match find_unescaped_slash(remainder) {
                Some(second) => (
                    pattern,
                    &remainder[..second],
                    &remainder[second + 1..],
                ),
                None => (pattern, remainder, ""),
            }
        }
        None => (body, "", ""),
    }
}

fn parse_flags(s: &str) -> SubFlags {
    let mut flags = SubFlags::default();
    for c in s.chars() {
        match c {
            'g' => flags.global = true,
            'i' => flags.ignore_case = Some(true),
            'I' => flags.ignore_case = Some(false),
            'n' => flags.no_replace = true,
            'e' => flags.quiet_empty = true,
            'c' => flags.confirm = true,
            _ => {}
        }
    }
    flags
}

/// `~`を直前の置換文字列（生テキスト）へ展開する。
/// エスケープされた文字はそのまま素通しする。
fn expand_tilde(raw: &str, previous: &str) -> String {
    let mut output = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            output.push('\\');
            if let Some(next) = chars.next() {
                output.push(next);
            }
        } else if c == '~' {
            output.push_str(previous);
        } else {
            output.push(c);
        }
    }
    output
}

fn flush_literal(
    parts: &mut Vec<ReplPart>,
    literal: &mut String,
) {
    if !literal.is_empty() {
        parts.push(ReplPart::Literal(std::mem::take(
            literal,
        )));
    }
}

/// 置換文字列をトークン列へ分解する。
fn parse_replacement(text: &str) -> Vec<ReplPart> {
    let mut parts = Vec::new();
    let mut literal = String::new();
    let mut chars = text.chars();

    while let Some(c) = chars.next() {
        if c == '&' {
            flush_literal(&mut parts, &mut literal);
            parts.push(ReplPart::Whole);
        } else if c == '\\' {
            match chars.next() {
                Some('0') => {
                    flush_literal(&mut parts, &mut literal);
                    parts.push(ReplPart::Whole);
                }
                Some(d) if d.is_ascii_digit() => {
                    flush_literal(&mut parts, &mut literal);
                    parts.push(ReplPart::Group(
                        d as u8 - b'0',
                    ));
                }
                Some('u') => {
                    flush_literal(&mut parts, &mut literal);
                    parts.push(ReplPart::UpperNext);
                }
                Some('l') => {
                    flush_literal(&mut parts, &mut literal);
                    parts.push(ReplPart::LowerNext);
                }
                Some('U') => {
                    flush_literal(&mut parts, &mut literal);
                    parts.push(ReplPart::UpperUntil);
                }
                Some('L') => {
                    flush_literal(&mut parts, &mut literal);
                    parts.push(ReplPart::LowerUntil);
                }
                Some('e') | Some('E') => {
                    flush_literal(&mut parts, &mut literal);
                    parts.push(ReplPart::EndCase);
                }
                // \r は木を壊すので対象外。バック
                // スラッシュごとそのまま素通しする。
                Some('r') => {
                    literal.push('\\');
                    literal.push('r');
                }
                Some(other) => literal.push(other),
                None => literal.push('\\'),
            }
        } else {
            literal.push(c);
        }
    }

    flush_literal(&mut parts, &mut literal);
    parts
}

fn push_transformed(
    output: &mut String,
    s: &str,
    case_mode: &mut Option<bool>,
    one_shot: &mut Option<bool>,
) {
    for c in s.chars() {
        let upper = one_shot.take().or(*case_mode);
        match upper {
            Some(true) => output.extend(c.to_uppercase()),
            Some(false) => output.extend(c.to_lowercase()),
            None => output.push(c),
        }
    }
}

/// キャプチャとトークン列から実際の置換文字列を作る。
fn build_replacement(
    caps: &Captures,
    parts: &[ReplPart],
) -> String {
    let mut output = String::new();
    let mut case_mode: Option<bool> = None;
    let mut one_shot: Option<bool> = None;

    for part in parts {
        match part {
            ReplPart::Literal(text) => push_transformed(
                &mut output,
                text,
                &mut case_mode,
                &mut one_shot,
            ),
            ReplPart::Whole => {
                let text = caps
                    .get(0)
                    .map(|m| m.as_str())
                    .unwrap_or("");
                push_transformed(
                    &mut output,
                    text,
                    &mut case_mode,
                    &mut one_shot,
                );
            }
            ReplPart::Group(n) => {
                let text = caps
                    .get(*n as usize)
                    .map(|m| m.as_str())
                    .unwrap_or("");
                push_transformed(
                    &mut output,
                    text,
                    &mut case_mode,
                    &mut one_shot,
                );
            }
            ReplPart::UpperNext => one_shot = Some(true),
            ReplPart::LowerNext => one_shot = Some(false),
            ReplPart::UpperUntil => case_mode = Some(true),
            ReplPart::LowerUntil => {
                case_mode = Some(false)
            }
            ReplPart::EndCase => case_mode = None,
        }
    }

    output
}

/// 空文字列マッチのまま足踏みしないよう、1文字分
/// 先へ進める。
fn advance_zero_width(text: &str, pos: usize) -> usize {
    match text[pos..].chars().next() {
        Some(c) => pos + c.len_utf8(),
        None => text.len() + 1,
    }
}

/// 1ノード分のテキストに対して置換する。
///
/// globalなら全マッチ、そうでなければ最初の1件のみ。
/// 探索位置は常に元のテキストの座標で進めるので、
/// 挿入した置換文字列を再び拾ってしまうことはない。
fn replace_all_in_text(
    text: &str,
    regex: &Regex,
    parts: &[ReplPart],
    global: bool,
) -> (String, usize, Option<usize>) {
    let mut result = String::with_capacity(text.len());
    let mut last = 0;
    let mut pos = 0;
    let mut count = 0;
    let mut last_start = None;

    loop {
        if pos > text.len() {
            break;
        }
        let Some(caps) = regex.captures_at(text, pos)
        else {
            break;
        };
        let m = caps.get(0).unwrap();
        let (start, end) = (m.start(), m.end());
        result.push_str(&text[last..start]);
        last_start = Some(result.len());
        result.push_str(&build_replacement(&caps, parts));
        count += 1;
        last = end;
        pos = if end > start {
            end
        } else {
            advance_zero_width(text, end)
        };
        if !global {
            break;
        }
    }

    result.push_str(&text[last..]);
    (result, count, last_start)
}

impl App {
    /// `:`コマンドの1行が`:s`/`:&`/`:&&`として解釈
    /// できればtrueを返し、実行まで済ませる。
    /// そうでなければfalseを返し、呼び出し元は既存の
    /// 分岐へフォールスルーする。
    pub(super) fn substitute_command(
        &mut self,
        line: &str,
    ) -> bool {
        let line = line.trim();
        let prefix_len = range_prefix_len(line);
        let range_text = &line[..prefix_len];
        let rest = &line[prefix_len..];

        let Some(range) = self.parse_range(range_text)
        else {
            return false;
        };

        if rest == "&" {
            self.repeat_substitute(range, false);
            return true;
        }
        if rest == "&&" {
            self.repeat_substitute(range, true);
            return true;
        }
        if let Some(body) = rest.strip_prefix("s/") {
            self.run_substitute(range, body);
            return true;
        }

        false
    }

    /// `g&`。全ノードへ、直前の検索パターンと直前の
    /// 置換（トークン列・flags）をそのまま再適用する。
    pub(super) fn global_repeat_substitute(&mut self) {
        let Some(last) = self.last_substitute.clone()
        else {
            self.message =
                "直前の置換がありません".to_string();
            return;
        };
        let Some((pattern, _)) =
            self.last_search.clone()
        else {
            self.message =
                "直前の置換がありません".to_string();
            return;
        };

        let range =
            (0, self.nodes.len().saturating_sub(1));
        self.execute_substitute(
            range,
            &pattern,
            last.parts,
            last.flags,
        );
    }

    fn repeat_substitute(
        &mut self,
        range: (usize, usize),
        keep_flags: bool,
    ) {
        let Some(last) = self.last_substitute.clone()
        else {
            self.message =
                "直前の置換がありません".to_string();
            return;
        };

        let flags = if keep_flags {
            last.flags
        } else {
            SubFlags::default()
        };

        self.execute_substitute(
            range,
            &last.pattern,
            last.parts,
            flags,
        );
    }

    fn run_substitute(
        &mut self,
        range: (usize, usize),
        body: &str,
    ) {
        let (pattern_raw, replacement_raw, flags_raw) =
            split_substitute_body(body);

        let pattern = if pattern_raw.is_empty() {
            match &self.last_search {
                Some((last, _)) => last.clone(),
                None => {
                    self.message =
                        "検索パターンがありません"
                            .to_string();
                    return;
                }
            }
        } else {
            let translated =
                translate_vim_pattern(pattern_raw);
            self.last_search =
                Some((translated.clone(), true));
            translated
        };

        let previous = self
            .last_substitute
            .as_ref()
            .map(|last| last.replacement_raw.clone())
            .unwrap_or_default();
        let expanded =
            expand_tilde(replacement_raw, &previous);
        let parts = parse_replacement(&expanded);
        let flags = parse_flags(flags_raw);

        self.last_substitute = Some(LastSubstitute {
            pattern: pattern.clone(),
            replacement_raw: expanded,
            parts: parts.clone(),
            flags: flags.clone(),
        });

        self.execute_substitute(
            range, &pattern, parts, flags,
        );
    }

    /// パターン・置換トークン列・flagsが揃った状態で
    /// 実際に検索・置換を行う。`:s`・`:&`・`:&&`・
    /// `g&`のいずれもここに集約する。
    fn execute_substitute(
        &mut self,
        range: (usize, usize),
        pattern: &str,
        parts: Vec<ReplPart>,
        flags: SubFlags,
    ) {
        let last = self.nodes.len().saturating_sub(1);
        let start = range.0.min(last);
        let end = range.1.min(last).max(start);

        let ignore_case = match flags.ignore_case {
            Some(value) => value,
            None => !pattern
                .chars()
                .any(|c| c.is_uppercase()),
        };

        let regex = match RegexBuilder::new(pattern)
            .case_insensitive(ignore_case)
            .build()
        {
            Ok(regex) => regex,
            Err(error) => {
                self.message = format!(
                    "検索パターンが不正です: {}",
                    error
                );
                return;
            }
        };

        if flags.no_replace {
            self.report_match_count(
                &regex,
                start,
                end,
                flags.quiet_empty,
            );
            return;
        }

        if flags.confirm {
            self.start_confirm(
                regex,
                parts,
                flags.global,
                start,
                end,
                flags.quiet_empty,
            );
            return;
        }

        self.apply_substitute(
            &regex,
            &parts,
            flags.global,
            start,
            end,
            flags.quiet_empty,
        );
    }

    fn report_match_count(
        &mut self,
        regex: &Regex,
        start: usize,
        end: usize,
        quiet_empty: bool,
    ) {
        let count: usize = (start..=end)
            .map(|node| {
                regex
                    .find_iter(&self.nodes[node].text)
                    .count()
            })
            .sum();

        if count == 0 {
            if !quiet_empty {
                self.message = format!(
                    "パターンが見つかりません: {}",
                    regex.as_str()
                );
            } else {
                self.message.clear();
            }
        } else {
            self.message =
                format!("{}箇所マッチしました", count);
        }
    }

    fn apply_substitute(
        &mut self,
        regex: &Regex,
        parts: &[ReplPart],
        global: bool,
        start: usize,
        end: usize,
        quiet_empty: bool,
    ) {
        self.begin_edit();
        let mut total = 0;
        let mut last_match = None;

        for node in start..=end {
            let text = self.nodes[node].text.clone();
            let (new_text, count, last_start) =
                replace_all_in_text(
                    &text, regex, parts, global,
                );
            if count > 0 {
                self.nodes[node].text = new_text;
                self.record();
                total += count;
                last_match = last_start.map(|col| (node, col));
            }
        }

        if total == 0 {
            if !quiet_empty {
                self.message = format!(
                    "パターンが見つかりません: {}",
                    regex.as_str()
                );
            } else {
                self.message.clear();
            }
            return;
        }

        if let Some((node, col)) = last_match {
            self.move_to(node);
            self.cursor_col = col;
            self.center_on_cursor();
        }
        self.message =
            format!("{}箇所を置換しました", total);
    }

    fn start_confirm(
        &mut self,
        regex: Regex,
        parts: Vec<ReplPart>,
        global: bool,
        start: usize,
        end: usize,
        quiet_empty: bool,
    ) {
        self.begin_edit();
        self.confirm_state = Some(ConfirmState {
            regex,
            parts,
            global,
            node: start,
            pos: 0,
            range_end: end,
            match_range: (0, 0),
            count: 0,
            quiet_empty,
            entered: false,
        });
        self.find_next();
    }

    /// `c`フラグの確認モード中のキー入力。
    pub(super) fn handle_confirm(
        &mut self,
        code: KeyCode,
    ) {
        match code {
            KeyCode::Char('y') => {
                self.apply_pending();
                self.find_next();
            }
            KeyCode::Char('n') => {
                self.skip_pending();
                self.find_next();
            }
            KeyCode::Char('a') => {
                self.apply_pending();
                while let Some((node, start, end)) =
                    self.locate_next()
                {
                    let state = self
                        .confirm_state
                        .as_mut()
                        .unwrap();
                    state.node = node;
                    state.match_range = (start, end);
                    self.apply_pending();
                }
                self.finish_confirm();
            }
            KeyCode::Char('l') => {
                self.apply_pending();
                self.finish_confirm();
            }
            KeyCode::Char('q') | KeyCode::Esc => {
                self.finish_confirm();
            }
            _ => {}
        }
    }

    /// 現在位置から次のマッチを探す。見つかれば
    /// カーソルを合わせてConfirmモードのまま
    /// trueを返し、無ければ締めてfalseを返す。
    fn find_next(&mut self) -> bool {
        match self.locate_next() {
            Some((node, start, end)) => {
                let state =
                    self.confirm_state.as_mut().unwrap();
                state.node = node;
                state.match_range = (start, end);
                state.entered = true;
                self.move_to(node);
                self.cursor_col = start;
                self.center_on_cursor();
                self.message =
                    "置換しますか？ (y/n/a/q/l)"
                        .to_string();
                self.mode = Mode::Confirm;
                true
            }
            None => {
                self.finish_confirm();
                false
            }
        }
    }

    /// state.node/posから、range_endまでの間で次の
    /// マッチを探す。空のノードはposを0に戻しつつ
    /// 読み飛ばす。
    fn locate_next(
        &mut self,
    ) -> Option<(usize, usize, usize)> {
        loop {
            let (node, pos, range_end) = {
                let state =
                    self.confirm_state.as_ref()?;
                (state.node, state.pos, state.range_end)
            };
            if node > range_end {
                return None;
            }

            let found = {
                let state =
                    self.confirm_state.as_ref().unwrap();
                let text = &self.nodes[node].text;
                if pos <= text.len() {
                    state
                        .regex
                        .find_at(text, pos)
                        .map(|m| (m.start(), m.end()))
                } else {
                    None
                }
            };

            if let Some((start, end)) = found {
                return Some((node, start, end));
            }

            let state =
                self.confirm_state.as_mut().unwrap();
            state.node += 1;
            state.pos = 0;
        }
    }

    fn apply_pending(&mut self) {
        let state = self.confirm_state.as_ref().unwrap();
        let node = state.node;
        let (start, end) = state.match_range;
        let global = state.global;
        let text = self.nodes[node].text.clone();
        let caps =
            state.regex.captures_at(&text, start).unwrap();
        let replacement =
            build_replacement(&caps, &state.parts);

        let mut new_text =
            String::with_capacity(text.len());
        new_text.push_str(&text[..start]);
        new_text.push_str(&replacement);
        new_text.push_str(&text[end..]);

        let mut new_pos = start + replacement.len();
        if new_pos == start {
            new_pos = advance_zero_width(&new_text, start);
        }

        self.nodes[node].text = new_text;
        self.record();
        self.move_to(node);
        self.cursor_col = start;
        self.center_on_cursor();

        let state = self.confirm_state.as_mut().unwrap();
        state.count += 1;
        if global {
            state.pos = new_pos;
        } else {
            state.node += 1;
            state.pos = 0;
        }
    }

    fn skip_pending(&mut self) {
        let state = self.confirm_state.as_ref().unwrap();
        let (start, end) = state.match_range;
        let global = state.global;
        let node = state.node;
        let text = &self.nodes[node].text;
        let next_pos = if end > start {
            end
        } else {
            advance_zero_width(text, start)
        };

        let state = self.confirm_state.as_mut().unwrap();
        if global {
            state.pos = next_pos;
        } else {
            state.node += 1;
            state.pos = 0;
        }
    }

    fn finish_confirm(&mut self) {
        let Some(state) = self.confirm_state.take()
        else {
            return;
        };
        self.mode = Mode::Normal;

        if !state.entered && state.count == 0 {
            if !state.quiet_empty {
                self.message = format!(
                    "パターンが見つかりません: {}",
                    state.regex.as_str()
                );
            } else {
                self.message.clear();
            }
        } else {
            self.message = format!(
                "{}箇所を置換しました",
                state.count
            );
        }
    }

    fn parse_range(
        &self,
        spec: &str,
    ) -> Option<(usize, usize)> {
        if spec.is_empty() {
            return Some((self.cursor, self.cursor));
        }
        if spec == "%" {
            return Some((
                0,
                self.nodes.len().saturating_sub(1),
            ));
        }

        let parts: Vec<&str> =
            spec.split(',').collect();
        match parts.as_slice() {
            [a] => {
                let n = self.resolve_atom(a)?;
                Some((n, n))
            }
            [a, b] => {
                let x = self.resolve_atom(a)?;
                let y = self.resolve_atom(b)?;
                if x <= y {
                    Some((x, y))
                } else {
                    Some((y, x))
                }
            }
            _ => None,
        }
    }

    fn resolve_atom(&self, s: &str) -> Option<usize> {
        match s {
            "." => Some(self.cursor),
            "$" => Some(
                self.nodes.len().saturating_sub(1),
            ),
            "" => None,
            _ => s
                .parse::<usize>()
                .ok()
                .map(|n| n.saturating_sub(1)),
        }
    }
}
