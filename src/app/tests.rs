use super::App;
use super::Mode;
use crate::node::nodes_from;
use crate::node::number_width;
use crate::reader;

use super::ui::draw;
use super::ui::title;

use std::fs;

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;

use ratatui::style::Modifier;
use ratatui::Terminal;

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

/// / ? n N * # と正規表現・smartcase・折り返し。
#[test]
fn search() {
    let mut app = App::new();
    press(&mut app, "iapple");
    for word in ["Banana", "cherry", "apple2", "date"]
    {
        press(&mut app, &format!("\n{}", word));
    }
    press(&mut app, "\x1b");
    assert_eq!(app.nodes.len(), 5);
    // apple(0) Banana(1) cherry(2) apple2(3) date(4)

    app.cursor = 0;

    // smartcase: 小文字だけのパターンは大文字小文字を
    // 無視する。
    press(&mut app, "/banana\n");
    assert_eq!(app.cursor, 1);

    // 正規表現として解釈される。
    press(&mut app, "/^date$\n");
    assert_eq!(app.cursor, 4);

    // 末尾から前方検索すると先頭へ折り返す。
    press(&mut app, "/apple\n");
    assert_eq!(app.cursor, 0);
    assert!(app.message.contains("折り返"));

    // n は同じ方向・同じパターンで繰り返す。
    press(&mut app, "n");
    assert_eq!(app.cursor, 3);
    assert!(!app.message.contains("折り返"));

    // N は逆方向だが、n・N を繰り返しても検索の
    // 基準方向（前方）自体は変わらない。
    //
    // x1(0) a(1) b(2) x2(3) c(4) x3(5) d(6)を使う。
    // カーソルをx2(3)に置いて前方検索し、Nで逆方向、
    // 続くnで前方に戻ることを確かめる。Nがlast_search
    // の方向を書き換えてしまう不具合があると、
    // 2回目のnも逆方向のまま止まってしまう。
    let mut app2 = insert("x1");
    for word in ["a", "b", "x2", "c", "x3", "d"] {
        press(&mut app2, &format!("\n{}", word));
    }
    press(&mut app2, "\x1b");
    app2.cursor = 0;
    press(&mut app2, "/x\n");
    assert_eq!(app2.cursor, 3);
    press(&mut app2, "N");
    assert_eq!(app2.cursor, 0);
    press(&mut app2, "n");
    assert_eq!(app2.cursor, 3);

    // * はカーソルのノードのテキストをそのまま
    // パターンにする。
    app.cursor = 4;
    press(&mut app, "*");
    // dateは1つしか無いので自分自身に戻ってくる。
    assert_eq!(app.cursor, 4);

    // ? は後方検索。
    app.cursor = 2;
    press(&mut app, "?apple\n");
    assert_eq!(app.cursor, 0);

    // 見つからなければメッセージを出し、カーソルは
    // 動かさない。
    app.cursor = 2;
    press(&mut app, "/nonexistentxyz\n");
    assert_eq!(app.cursor, 2);
    assert!(app.message.contains("見つかりません"));

    // 不正な正規表現もメッセージを出す。
    app.cursor = 2;
    press(&mut app, "/[\n");
    assert_eq!(app.cursor, 2);
    assert!(app.message.contains("不正"));

    // Escで検索を取り消す。
    press(&mut app, "/apple");
    assert_eq!(app.mode, Mode::Search);
    press(&mut app, "\x1b");
    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(app.cursor, 2);
}

/// vimの既定`/`に合わせ、? + ( ) { } | はエスケープ
/// 無しならリテラル、エスケープすると特殊になる。
/// \/ はエスケープしても文字通りのスラッシュ。
#[test]
fn search_vim_pattern_translation() {
    let mut app = insert("start");
    for word in ["lis", "list?", "a(b)c", "a/b"] {
        press(&mut app, &format!("\n{}", word));
    }
    press(&mut app, "\x1b");
    assert_eq!(app.nodes.len(), 5);
    // start(0) lis(1) list?(2) a(b)c(3) a/b(4)

    // エスケープ無しの ? はリテラル。"lis" は
    // 素通りして "list?" だけにヒットする。
    app.cursor = 0;
    press(&mut app, "/list?\n");
    assert_eq!(app.cursor, 2);

    // \? はvimと同じくRustの特殊文字になる
    // （直前の1文字が0か1回）。"list?" より手前の
    // "lis" がヒットする。
    app.cursor = 0;
    press(&mut app, "/list\\?\n");
    assert_eq!(app.cursor, 1);

    // エスケープ無しの ( ) もリテラル。
    // 変換していなければグループ化として扱われ、
    // "a(b)c" ではなく "abc" を探すことになり
    // 見つからない。
    app.cursor = 0;
    press(&mut app, "/a(b)c\n");
    assert_eq!(app.cursor, 3);

    // \/ はエスケープしても文字通りのスラッシュ
    // として検索できる（vimと同じ書き方が使える）。
    app.cursor = 0;
    press(&mut app, "/a\\/b\n");
    assert_eq!(app.cursor, 4);

    // \= は\?のvimでの別表記で、同じ意味になる。
    app.cursor = 0;
    press(&mut app, "/list\\=\n");
    assert_eq!(app.cursor, 1);

    // \a はvimでは英字1文字。数字だけの"123"は
    // 飛ばして、英字の"word"にヒットする。ベル文字
    // (0x07)というRust regexの本来の意味のままだと
    // どちらにもヒットしない。
    let mut app3 = insert("123");
    press(&mut app3, "\nword");
    press(&mut app3, "\x1b");
    app3.cursor = 0;
    press(&mut app3, "/\\a\n");
    assert_eq!(app3.cursor, 1);
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



/// F2で切り替えるとカーソルが画面の中央に来る
/// よう自動でスクロールする。
#[test]
fn f2_centers_cursor() {
    let mut terminal = Terminal::new(
        ratatui::backend::TestBackend::new(20, 10),
    )
    .unwrap();
    let mut app = App::new();
    press(&mut app, "i0");
    for n in 1..30 {
        press(&mut app, &format!("\n{}", n));
    }
    press(&mut app, "\x1b");
    // 高さ7の画面。20番目のノードへ移動してからF2。
    screen(&mut terminal, &mut app);
    press(&mut app, ":20\n");
    app.toggle_source_view();
    // scroll + height/2 が着地したノード番号に近い
    // （画面の中央付近に来る）。
    assert_eq!(app.scroll + app.height / 2, app.cursor);

    // 木に戻すときも同様。
    app.toggle_source_view();
    assert_eq!(app.scroll + app.height / 2, app.cursor);
}

/// 検索でヒットしたノードもF2と同じく画面中央へ
/// 寄る。末尾の方でヒットしても下端に埋もれない。
#[test]
fn search_centers_cursor() {
    let mut terminal = Terminal::new(
        ratatui::backend::TestBackend::new(20, 10),
    )
    .unwrap();
    let mut app = App::new();
    press(&mut app, "i0");
    for n in 1..30 {
        press(&mut app, &format!("\n{}", n));
    }
    press(&mut app, "\x1b");
    // 高さ7の画面。先頭から末尾付近の"25"を検索。
    screen(&mut terminal, &mut app);
    app.cursor = 0;
    press(&mut app, "/25\n");
    assert_eq!(app.cursor, 25);
    assert_eq!(app.scroll + app.height / 2, app.cursor);

    // n で移動したときも同様。
    press(&mut app, ":0\n");
    press(&mut app, "n");
    assert_eq!(app.cursor, 25);
    assert_eq!(app.scroll + app.height / 2, app.cursor);
}

/// 折り返しのある長い行を含む木の末尾でF2を押しても
/// カーソルが画面外に消えない。
///
/// ratatuiのParagraph::scroll()は折り返し有効時、
/// yをノード番号ではなく折り返し後の画面行として
/// 解釈する。scroll/height/カーソル追従の単位を
/// row_offsets（画面行）にそろえていないと、
/// この境界でカーソルがどの行にも描画されなくなる。
#[test]
fn f2_at_bottom_keeps_cursor_visible() {
    let text = std::fs::read_to_string(
        "test/test.scm",
    )
    .unwrap();
    let reading = reader::read(&text).unwrap();
    let mut app = App::new();
    app.nodes = nodes_from(&reading.data);
    app.cursor = app.nodes.len() - 1;

    let mut terminal = Terminal::new(
        ratatui::backend::TestBackend::new(40, 20),
    )
    .unwrap();
    terminal
        .draw(|frame| draw(frame, &mut app))
        .unwrap();
    app.toggle_source_view();
    terminal
        .draw(|frame| draw(frame, &mut app))
        .unwrap();

    let buffer = terminal.backend().buffer();
    let visible = buffer.content.iter().any(|cell| {
        cell.modifier
            .contains(ratatui::style::Modifier::REVERSED)
    });
    assert!(
        visible,
        "カーソルのセルが画面のどこにも見つかりません"
    );
}
