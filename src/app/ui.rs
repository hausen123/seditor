use super::App;
use super::Mode;
use super::visual::VisualKind;
use crate::node::number_width;

use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::text::Text;
use ratatui::widgets::Block;
use ratatui::widgets::Borders;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Wrap;
use ratatui::Frame;

pub(crate) fn draw(
    frame: &mut Frame,
    app: &mut App,
) {
    // 下1行をコマンドとメッセージに使う。
    let areas = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(frame.area());

    // 枠の上下を除いた行数。
    // 画面の高さは描画時にしか分からない。
    app.height =
        areas[0].height.saturating_sub(2) as usize;

    // 枠と行番号ガターを除いた横の文字数。
    let gutter = if app.number {
        number_width(app.nodes.len())
    } else {
        0
    };
    app.width = (areas[0].width as usize)
        .saturating_sub(2)
        .saturating_sub(gutter);

    // scroll/カーソル追従の単位（画面行）を最新にする。
    // ソース表示は折り返すので、1ノードが複数行になる
    // ことがある。
    app.refresh_row_offsets(
        areas[0].width.saturating_sub(2),
    );

    // ソース表示も木と同じ App::nodes/cursor を使う
    // （F2でdepth:0の行に変換してある）ので、
    // カーソル追従・行番号・カーソル反転は共通。
    // 横スクロールだけソース表示では不要（折り返す）。
    app.follow_cursor();
    if !app.source_mode {
        app.follow_cursor_horizontal();
    }

    let text: Text = app.tree_display().into();

    let mut paragraph = Paragraph::new(text)
        .scroll((app.scroll as u16, 0));

    // ソース表示は折り返す。カーソル行という概念が
    // 無いので、木の表示のような横スクロールは
    // 持ち込まず素直に折り返せる。
    if app.source_mode {
        paragraph = paragraph
            .wrap(Wrap { trim: false });
    }

    let paragraph = paragraph
        .block(
            Block::default()
                .title(title(app))
                .borders(Borders::ALL),
        );

    frame.render_widget(paragraph, areas[0]);

    let status = if app.mode == Mode::Command {
        format!(":{}", app.command)
    } else if app.mode == Mode::Search {
        let prefix =
            if app.search_forward { '/' } else { '?' };
        format!("{}{}", prefix, app.search)
    } else {
        app.message.clone()
    };

    frame.render_widget(
        Paragraph::new(status),
        areas[1],
    );
}

/// ファイル名、変更の有無、モードを並べた見出し。
pub(super) fn title(app: &App) -> String {
    let name = match &app.path {
        Some(path) => path.display().to_string(),
        None => "[No Name]".to_string(),
    };

    let mark = if app.modified { " [+]" } else { "" };

    // ソース表示中も編集モードを出す。中身を見れば
    // ソース表示かどうかは分かるので、そこは出さない。
    let mode = match app.mode {
        Mode::Normal => "NORMAL",
        Mode::Insert => "INSERT",
        Mode::Command => "COMMAND",
        Mode::Search => "SEARCH",
        Mode::Confirm => "CONFIRM",
        Mode::Visual => match app.visual_kind {
            VisualKind::Line => "VISUAL LINE",
            VisualKind::Char => "VISUAL",
        },
    };

    format!(" {}{} - {} ", name, mark, mode)
}
