use crate::reader::Datum;

use ratatui::text::Span;
use ratatui::style::Modifier;
use ratatui::style::Style;

pub const WIDTH: usize = 80;

/// 子を並べる位置の決め方。
pub enum Indent {
    /// 先頭のn個を見出し行に残し、残りを+2桁に置く。
    Body(usize),
    /// 子を第1子の桁に揃える。
    Align,
}

/// 幅に収まっても必ず改行する形。
pub fn always_break(text: &str) -> bool {
    matches!(
        text,
        "define" | "lambda" | "let" | "let*" | "letrec"
            | "letrec*" | "when" | "unless" | "case"
            | "do" | "begin" | "cond"
    )
}

/// outputの末尾がいま何桁目にあるか。
pub fn current_column(output: &str) -> usize {
    match output.rfind('\n') {
        Some(position) => {
            output[position + 1..].chars().count()
        }
        None => output.chars().count(),
    }
}

#[derive(Debug, Clone)]
pub struct Node {
    pub text: String,
    pub depth: usize,
}

/// 読み取ったデータをノード列に直す。
pub fn nodes_from(data: &[Datum]) -> Vec<Node> {
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

/// マーカーの連なりがアトム1つに行き着くなら、
/// 印の文字を連結したテキストを返す。
///
/// '`x のように印が複数重なっていても、最終的に
/// アトムなら1ノードに畳めるため。リストや
/// コメントに行き着く場合はNone。
fn atom_chain(datum: &Datum) -> Option<String> {
    match datum {
        Datum::Atom(text) => Some(text.clone()),
        Datum::Marker(mark, child) => {
            atom_chain(child)
                .map(|rest| format!("{}{}", mark, rest))
        }
        _ => None,
    }
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
            emit_marker(mark, child, depth, nodes);
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

/// マーカーを書き出す。emit()とemit_quoted()の
/// どちらから呼ばれても同じ規則になる。
///
/// アトムに行き着くなら1ノードに畳む。リストなら、
/// 空リストだけ例外的に◦を挟み、それ以外は◦を
/// 挟まずマーカー自身が要素を直接子に持つ（クォートの
/// 中は呼び出しではなくただのデータの並びなので、
/// 先頭を見出しにする理由が無い）。マーカーが
/// 続く場合はそのまま連結する。
fn emit_marker(
    mark: &str,
    child: &Datum,
    depth: usize,
    nodes: &mut Vec<Node>,
) {
    if let Some(rest) = atom_chain(child) {
        nodes.push(Node {
            text: format!("{}{}", mark, rest),
            depth,
        });
        return;
    }

    nodes.push(Node {
        text: mark.to_string(),
        depth,
    });

    match child {
        Datum::List(items) if items.is_empty() => {
            // '() だけ例外的に◦を挟む。
            nodes.push(Node {
                text: String::new(),
                depth: depth + 1,
            });
        }
        Datum::List(items) => {
            for item in items {
                emit_quoted(item, depth + 1, nodes);
            }
        }
        _ => emit(child, depth + 1, nodes),
    }
}

/// クォートの中を書き出す。emit()と違い、リストは
/// 先頭がアトムでも見出しにせず、常に◦にする
/// （マーカーの付いていない入れ子のリストには
/// attach先が無いため）。
fn emit_quoted(
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
            emit_marker(mark, child, depth, nodes);
        }
        Datum::List(items) => {
            nodes.push(Node {
                text: String::new(),
                depth,
            });
            for item in items {
                emit_quoted(item, depth + 1, nodes);
            }
        }
    }
}

/// 深さをブロック内の最小に合わせて0からにする。
///
/// 先頭を基準にすると、後ろに浅いノードが続く場合に
/// 負になってしまう。
pub fn normalize(nodes: &[Node]) -> Vec<Node> {
    let base = nodes
        .iter()
        .map(|node| node.depth)
        .min()
        .unwrap_or(0);

    nodes
        .iter()
        .map(|node| Node {
            text: node.text.clone(),
            depth: node.depth - base,
        })
        .collect()
}

/// 行番号の最小の桁数。
///
/// ノード数が9→10と増えるたびに桁が変わると
/// 木全体が横にずれるので、下限を決めておく。
const NUMBER_WIDTH: usize = 3;

/// count行に行番号を振るときの桁幅。空白1つを含む。
pub fn number_width(count: usize) -> usize {
    let digits = count.to_string().len();
    digits.max(NUMBER_WIDTH) + 1
}

/// 淡く表示する行番号。
pub fn number_span(
    line: usize,
    width: usize,
) -> Span<'static> {
    Span::styled(
        format!("{:>1$} ", line, width - 1),
        Style::default().add_modifier(Modifier::DIM),
    )
}
