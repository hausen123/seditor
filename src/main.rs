mod app;
mod node;
mod reader;

use app::draw;
use app::App;

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use crossterm::{
    event::{
        self,
        DisableBracketedPaste,
        EnableBracketedPaste,
        Event,
        KeyEventKind,
    },
    execute,
    terminal::{
        disable_raw_mode,
        enable_raw_mode,
        EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};

use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

fn main() -> io::Result<()> {
    enable_raw_mode()?;

    let mut stdout = io::stdout();

    execute!(
        stdout,
        EnterAlternateScreen,
        EnableBracketedPaste
    )?;

    let backend =
        CrosstermBackend::new(stdout);

    let mut terminal =
        Terminal::new(backend)?;

    let path = std::env::args()
        .nth(1)
        .map(PathBuf::from);

    let result =
        run(&mut terminal, path);

    disable_raw_mode()?;

    execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        LeaveAlternateScreen
    )?;

    terminal.show_cursor()?;

    result
}

fn run(
    terminal: &mut Terminal<
        CrosstermBackend<io::Stdout>
    >,
    path: Option<PathBuf>,
) -> io::Result<()> {
    let mut app = App::new();

    if let Some(path) = path {
        app.open(path);
    }

    loop {
        terminal.draw(|frame| {
            draw(frame, &mut app);
        })?;

        if event::poll(
            Duration::from_millis(50)
        )? {
            match event::read()? {
                Event::Key(key) => {
                    // 離したときのイベントを送る端末が
                    // あるので、押した分だけ処理する。
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }
                    if !app.handle_key(key) {
                        break;
                    }
                }
                Event::Paste(text) => {
                    app.handle_paste(&text);
                }
                _ => {}
            }
        }
    }

    Ok(())
}
