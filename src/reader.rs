//! S式のリーダ。
//!
//! 前置記号は(quote x)に展開せず印として保つ。
//! 展開すると書き戻したとき元のファイルと変わって
//! しまうため。
//!
//! コメントは読み終えたトップレベルの式の直前に
//! まとめて出す。式の中に書かれていたものも
//! 深さ0に出るので、内容は失われるが位置はずれる。

#[derive(Debug, Clone, PartialEq)]
pub enum Datum {
    Atom(String),
    List(Vec<Datum>),
    /// ' や #( のように直後の1データに付く記号。
    Marker(String, Box<Datum>),
    /// ; や #| |# のコメント。
    Comment(String),
}

/// 読み取った結果と、式の中から抜き出した
/// コメントの数。
pub struct Reading {
    pub data: Vec<Datum>,
    pub hoisted: usize,
}

pub fn read(text: &str) -> Result<Reading, String> {
    let mut reader = Reader {
        chars: text.chars().collect(),
        position: 0,
        depth: 0,
        comments: Vec::new(),
        hoisted: 0,
    };

    let data = reader.read_all()?;

    Ok(Reading {
        data,
        hoisted: reader.hoisted,
    })
}

/// シンボルの終わりになる文字。
///
/// [ ] は区切りに含めない。Gaucheの #[a-z] や
/// #/regexp/ を1つのアトムとして読むため。
fn is_delimiter(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '(' | ')' | '"' | ';' | '\'' | '`' | ',' | '|'
        )
}

struct Reader {
    chars: Vec<char>,
    position: usize,
    /// 今いる括弧の深さ。コメントの抜き出しを数える。
    depth: usize,
    /// まだ出していないコメント。
    comments: Vec<String>,
    hoisted: usize,
}

impl Reader {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.position).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.position + offset).copied()
    }

    fn skip_space(&mut self) {
        while matches!(self.peek(), Some(c) if c.is_whitespace())
        {
            self.position += 1;
        }
    }

    fn slice(&self, start: usize) -> String {
        self.chars[start..self.position].iter().collect()
    }

    /// 溜まったコメントを式の直前に出す。
    fn flush(&mut self, data: &mut Vec<Datum>) {
        for text in self.comments.drain(..) {
            data.push(Datum::Comment(text));
        }
    }

    fn read_all(&mut self) -> Result<Vec<Datum>, String> {
        let mut data = Vec::new();

        loop {
            self.skip_space();

            match self.peek() {
                None => break,
                Some(')') => {
                    return Err(
                        "閉じ括弧が多すぎます".to_string()
                    );
                }
                _ => {
                    let datum = self.read_datum()?;
                    self.flush(&mut data);
                    if let Some(datum) = datum {
                        data.push(datum);
                    }
                }
            }
        }

        self.flush(&mut data);

        Ok(data)
    }

    /// 1つのデータを読む。
    ///
    /// コメントだけを読んだときはNoneを返す。
    /// コメントはcommentsに溜めて後でまとめて出す。
    fn read_datum(
        &mut self,
    ) -> Result<Option<Datum>, String> {
        self.skip_space();

        let Some(c) = self.peek() else {
            return Err("データがありません".to_string());
        };

        match c {
            '(' => {
                self.position += 1;
                Ok(Some(self.read_list()?))
            }
            ';' => {
                let text = self.read_line_comment();
                self.push_comment(text);
                Ok(None)
            }
            '\'' | '`' => {
                self.position += 1;
                Ok(Some(self.read_marker(&c.to_string())?))
            }
            ',' => {
                self.position += 1;
                let mark = if self.peek() == Some('@') {
                    self.position += 1;
                    ",@"
                } else {
                    ","
                };
                Ok(Some(self.read_marker(mark)?))
            }
            '"' => Ok(Some(self.read_string()?)),
            '|' => Ok(Some(self.read_pipe()?)),
            '#' => self.read_hash(),
            _ => Ok(Some(Datum::Atom(self.read_symbol()))),
        }
    }

    fn push_comment(&mut self, text: String) {
        if self.depth > 0 {
            self.hoisted += 1;
        }
        self.comments.push(text);
    }

    fn read_list(&mut self) -> Result<Datum, String> {
        let mut items = Vec::new();
        self.depth += 1;

        loop {
            self.skip_space();

            match self.peek() {
                None => {
                    return Err(
                        "括弧が閉じていません".to_string()
                    );
                }
                Some(')') => {
                    self.position += 1;
                    self.depth -= 1;
                    return Ok(Datum::List(items));
                }
                _ => {
                    if let Some(datum) =
                        self.read_datum()?
                    {
                        items.push(datum);
                    }
                }
            }
        }
    }

    /// 印の後ろの1データを読む。
    ///
    /// 間にコメントがあっても飛ばして探す。
    fn read_marker(
        &mut self,
        mark: &str,
    ) -> Result<Datum, String> {
        loop {
            if let Some(datum) = self.read_datum()? {
                return Ok(Datum::Marker(
                    mark.to_string(),
                    Box::new(datum),
                ));
            }
        }
    }

    fn read_symbol(&mut self) -> String {
        let start = self.position;

        while matches!(self.peek(), Some(c) if !is_delimiter(c))
        {
            self.position += 1;
        }

        self.slice(start)
    }

    fn read_line_comment(&mut self) -> String {
        let start = self.position;

        while matches!(self.peek(), Some(c) if c != '\n') {
            self.position += 1;
        }

        self.slice(start)
    }

    fn read_string(&mut self) -> Result<Datum, String> {
        let start = self.position;
        self.position += 1;

        loop {
            match self.peek() {
                None => {
                    return Err(
                        "文字列が閉じていません".to_string()
                    );
                }
                Some('\\') => self.position += 2,
                Some('"') => {
                    self.position += 1;
                    break;
                }
                _ => self.position += 1,
            }
        }

        Ok(Datum::Atom(self.slice(start)))
    }

    fn read_pipe(&mut self) -> Result<Datum, String> {
        let start = self.position;
        self.position += 1;

        loop {
            match self.peek() {
                None => {
                    return Err(
                        "|が閉じていません".to_string()
                    );
                }
                Some('|') => {
                    self.position += 1;
                    break;
                }
                _ => self.position += 1,
            }
        }

        Ok(Datum::Atom(self.slice(start)))
    }

    /// #で始まるもの。
    fn read_hash(
        &mut self,
    ) -> Result<Option<Datum>, String> {
        match self.peek_at(1) {
            // ベクタ
            Some('(') => {
                self.position += 2;
                let list = self.read_list()?;
                Ok(Some(Datum::Marker(
                    "#".to_string(),
                    Box::new(list),
                )))
            }
            // ブロックコメント
            Some('|') => {
                let text = self.read_block_comment()?;
                self.push_comment(text);
                Ok(None)
            }
            // データコメント
            Some(';') => {
                self.position += 2;
                Ok(Some(self.read_marker("#;")?))
            }
            // 文字リテラル
            Some('\\') => {
                Ok(Some(Datum::Atom(self.read_character())))
            }
            // バイトベクタ
            Some('u')
                if self.peek_at(2) == Some('8')
                    && self.peek_at(3) == Some('(') =>
            {
                self.position += 4;
                let list = self.read_list()?;
                Ok(Some(Datum::Marker(
                    "#u8".to_string(),
                    Box::new(list),
                )))
            }
            _ => {
                // #0= はラベル。#0# や #t はアトム。
                let mut offset = 1;
                while matches!(
                    self.peek_at(offset),
                    Some(c) if c.is_ascii_digit()
                ) {
                    offset += 1;
                }
                if offset > 1
                    && self.peek_at(offset) == Some('=')
                {
                    let start = self.position;
                    self.position += offset + 1;
                    let label = self.slice(start);
                    return Ok(Some(
                        self.read_marker(&label)?,
                    ));
                }
                Ok(Some(Datum::Atom(self.read_symbol())))
            }
        }
    }

    /// #\a や #\space。
    ///
    /// #\( や #\; のように区切り文字そのものが
    /// 来ることがあるので、1文字目は無条件に取る。
    fn read_character(&mut self) -> String {
        let start = self.position;
        self.position += 2;

        if let Some(c) = self.peek() {
            self.position += 1;

            if c.is_alphanumeric() {
                while matches!(
                    self.peek(),
                    Some(c) if c.is_alphanumeric() || c == '-'
                ) {
                    self.position += 1;
                }
            }
        }

        self.slice(start)
    }

    /// #| |# は入れ子にできる。
    fn read_block_comment(
        &mut self,
    ) -> Result<String, String> {
        let start = self.position;
        self.position += 2;
        let mut level = 1;

        while level > 0 {
            match (self.peek(), self.peek_at(1)) {
                (None, _) => {
                    return Err(
                        "ブロックコメントが閉じていません"
                            .to_string(),
                    );
                }
                (Some('#'), Some('|')) => {
                    self.position += 2;
                    level += 1;
                }
                (Some('|'), Some('#')) => {
                    self.position += 2;
                    level -= 1;
                }
                _ => self.position += 1,
            }
        }

        Ok(self.slice(start))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(text: &str) -> Vec<Datum> {
        read(text).unwrap().data
    }

    fn atom(text: &str) -> Datum {
        Datum::Atom(text.to_string())
    }

    #[test]
    fn atoms() {
        assert_eq!(data("x"), vec![atom("x")]);
        assert_eq!(data("-1"), vec![atom("-1")]);
        assert_eq!(data("#t #f"), vec![atom("#t"), atom("#f")]);
        assert_eq!(data("#!fold-case"), vec![atom("#!fold-case")]);
        assert_eq!(data("..."), vec![atom("...")]);
        assert_eq!(data("."), vec![atom(".")]);
        // Gaucheの文字集合と正規表現。
        assert_eq!(data("#[a-z]"), vec![atom("#[a-z]")]);
        assert_eq!(data("#/ab+/"), vec![atom("#/ab+/")]);
    }

    #[test]
    fn characters() {
        // 区切り文字そのものが来る場合。
        assert_eq!(data(r"#\a"), vec![atom(r"#\a")]);
        assert_eq!(data(r"#\("), vec![atom(r"#\(")]);
        assert_eq!(data(r"#\;"), vec![atom(r"#\;")]);
        assert_eq!(data(r"#\ "), vec![atom(r"#\ ")]);
        assert_eq!(data(r"#\space"), vec![atom(r"#\space")]);
    }

    #[test]
    fn strings_and_pipes() {
        assert_eq!(
            data(r#""a b""#),
            vec![atom(r#""a b""#)]
        );
        // エスケープした引用符で終わらない。
        assert_eq!(
            data(r#""a\"b" x"#),
            vec![atom(r#""a\"b""#), atom("x")]
        );
        // 文字列の中の括弧やセミコロン。
        assert_eq!(
            data(r#""(a ; b)""#),
            vec![atom(r#""(a ; b)""#)]
        );
        assert_eq!(
            data("|foo bar|"),
            vec![atom("|foo bar|")]
        );
    }

    #[test]
    fn lists() {
        assert_eq!(data("()"), vec![Datum::List(vec![])]);
        assert_eq!(
            data("(a b)"),
            vec![Datum::List(vec![atom("a"), atom("b")])]
        );
        assert_eq!(
            data("(a . b)"),
            vec![Datum::List(vec![
                atom("a"),
                atom("."),
                atom("b")
            ])]
        );
    }

    #[test]
    fn markers() {
        assert_eq!(
            data("'(a b)"),
            vec![Datum::Marker(
                "'".to_string(),
                Box::new(Datum::List(vec![
                    atom("a"),
                    atom("b")
                ]))
            )]
        );
        assert_eq!(
            data("#(1 2)"),
            vec![Datum::Marker(
                "#".to_string(),
                Box::new(Datum::List(vec![
                    atom("1"),
                    atom("2")
                ]))
            )]
        );
        assert_eq!(
            data(",@x"),
            vec![Datum::Marker(
                ",@".to_string(),
                Box::new(atom("x"))
            )]
        );
        assert_eq!(
            data("#0=(a)"),
            vec![Datum::Marker(
                "#0=".to_string(),
                Box::new(Datum::List(vec![atom("a")]))
            )]
        );
        // quoteに展開しない。
        assert!(!matches!(
            &data("'x")[0],
            Datum::List(_)
        ));
    }

    #[test]
    fn comments() {
        // トップレベルのコメントは順番のまま。
        assert_eq!(
            data("; head\nx"),
            vec![
                Datum::Comment("; head".to_string()),
                atom("x")
            ]
        );
        // 式の中のものは式の直前に出る。
        let reading = read("(a ; note\n b)").unwrap();
        assert_eq!(reading.hoisted, 1);
        assert_eq!(
            reading.data,
            vec![
                Datum::Comment("; note".to_string()),
                Datum::List(vec![atom("a"), atom("b")])
            ]
        );
        // 入れ子のブロックコメント。
        let reading =
            read("#| a #| b |# c |# x").unwrap();
        assert_eq!(reading.hoisted, 0);
        assert_eq!(
            reading.data,
            vec![
                Datum::Comment(
                    "#| a #| b |# c |#".to_string()
                ),
                atom("x")
            ]
        );
        // #; はコメントではなく印。
        assert_eq!(
            data("#;x"),
            vec![Datum::Marker(
                "#;".to_string(),
                Box::new(atom("x"))
            )]
        );
    }

    #[test]
    fn errors() {
        assert!(read("(a").is_err());
        assert!(read("a)").is_err());
        assert!(read("\"a").is_err());
        assert!(read("#| a").is_err());
    }
}
