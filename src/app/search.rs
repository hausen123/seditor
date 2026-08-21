use super::App;

/// 検索パターンを、vimの既定（magicモード）の感覚に
/// 合わせてRustのregex構文へ書き換える。
///
/// vimの既定では `? + ( ) { } |` はエスケープなしだと
/// リテラルで、`\?` `\+` `\(` `\)` `\{` `\}` `\|` と
/// 書いたときだけ特殊な意味になる。Rustのregexはその
/// 逆（エスケープなしで特殊、エスケープするとリテラル）
/// なので、この7文字だけ意味を反転させる。
///
/// `\=` は`\?`のvimでの別表記なので同じ意味に変換する。
/// `\a`はvimでは英字1文字を表すが、Rustの`\a`はベル文字
/// (0x07)という別物になってしまうので、同じ意味のPOSIX
/// クラス`[[:alpha:]]`に展開する。
///
/// それ以外の文字（`.` `*` `^` `$` `[` `]` や `\d` の
/// ようなエスケープ列）はvimとRustで扱いが同じなので
/// そのまま通す。
fn translate_vim_pattern(pattern: &str) -> String {
    const SWAP: [char; 7] =
        ['?', '+', '(', ')', '{', '}', '|'];

    let mut output = String::with_capacity(pattern.len());
    let mut chars = pattern.chars();

    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                // \? \+ \( \) \{ \} \| はvimでは特殊。
                // バックスラッシュを外し、Rustの
                // メタ文字として機能させる。
                Some(next) if SWAP.contains(&next) => {
                    output.push(next);
                }
                // \= は\?と同じ（0か1回）のvimの別表記。
                Some('=') => output.push('?'),
                // \a はvimでは英字1文字。Rustには
                // 対応する短縮記法が無く、素通しすると
                // ベル文字(0x07)という別物になって
                // しまうので、同じ意味のPOSIXクラスに
                // 展開する。
                Some('a') => {
                    output.push_str("[[:alpha:]]")
                }
                // それ以外の \X はvimとRustで意味が
                // 変わらないのでそのまま通す
                // （\. \d \\ \/ など）。
                Some(next) => {
                    output.push('\\');
                    output.push(next);
                }
                None => output.push('\\'),
            }
        } else if SWAP.contains(&c) {
            // エスケープの無い ? + ( ) { } | はvimでは
            // リテラル。Rustでリテラルにするには
            // エスケープが要る。
            output.push('\\');
            output.push(c);
        } else {
            output.push(c);
        }
    }

    output
}

impl App {
    /// / や ? の入力を確定したときに呼ぶ。
    ///
    /// 空文字なら直前のパターンを使う（vimの`//`と同じ）。
    /// 見つかったパターンは n・N のために覚えておく。
    pub(super) fn search_and_move(
        &mut self,
        pattern: &str,
        forward: bool,
    ) {
        let pattern = if pattern.is_empty() {
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
            translate_vim_pattern(pattern)
        };

        self.last_search =
            Some((pattern.clone(), forward));
        self.perform_search(&pattern, forward);
    }

    /// n・N で直前のパターンを繰り返す。
    ///
    /// same_directionがfalseなら逆方向。繰り返すたびに
    /// 基準の方向が変わらないよう、last_searchはここでは
    /// 更新しない。
    pub(super) fn repeat_search(&mut self, same_direction: bool) {
        let Some((pattern, forward)) =
            self.last_search.clone()
        else {
            self.message =
                "検索パターンがありません".to_string();
            return;
        };

        let direction = if same_direction {
            forward
        } else {
            !forward
        };

        self.perform_search(&pattern, direction);
    }

    /// * と # 。カーソルのノードのテキストをそのまま
    /// （正規表現として特別扱いせず）パターンにする。
    pub(super) fn search_word(&mut self, forward: bool) {
        let text = self.text().to_string();

        if text.is_empty() {
            self.message =
                "空のノードは検索できません".to_string();
            return;
        }

        let pattern = regex::escape(&text);
        self.last_search =
            Some((pattern.clone(), forward));
        self.perform_search(&pattern, forward);
    }

    /// 実際にパターンをコンパイルしてカーソルを動かす。
    ///
    /// 大文字を含まないパターンは大文字小文字を無視する
    /// （smartcase）。末尾/先頭まで探して見つからなければ
    /// 逆側から折り返す（wrapscan）。
    fn perform_search(
        &mut self,
        pattern: &str,
        forward: bool,
    ) {
        let ignore_case =
            !pattern.chars().any(|c| c.is_uppercase());

        let regex = match regex::RegexBuilder::new(pattern)
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

        let len = self.nodes.len();
        let start = self.cursor;

        let found = (1..=len).map(|offset| {
            if forward {
                (start + offset) % len
            } else {
                (start + len - offset) % len
            }
        }).find(|&index| {
            regex.is_match(&self.nodes[index].text)
        });

        let Some(index) = found else {
            self.message = format!(
                "パターンが見つかりません: {}",
                pattern
            );
            return;
        };

        let wrapped = if forward {
            index < start
        } else {
            index > start
        };

        self.move_to(index);
        // F2と同じく、ヒットしたノードが画面の下端に
        // 埋もれて見づらくならないよう中央へ寄せる。
        self.center_on_cursor();

        self.message = if wrapped && forward {
            "検索は末尾から先頭へ折り返しました"
                .to_string()
        } else if wrapped {
            "検索は先頭から末尾へ折り返しました"
                .to_string()
        } else {
            String::new()
        };
    }
}
