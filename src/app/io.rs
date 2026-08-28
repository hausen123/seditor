use super::view::render_tree_lines;
use super::App;
use crate::node::nodes_from;
use crate::reader;

use std::fs;
use std::path::Path;
use std::path::PathBuf;

impl App {
    /// :で始まる1行を実行する。
    pub(super) fn run_command(&mut self, line: &str) {
        // :s / :& / :&& は空白やスラッシュを含みうる
        // ので、split_whitespaceより先に専用パーサへ
        // 通す。
        if self.substitute_command(line) {
            return;
        }

        let mut words = line.split_whitespace();

        let Some(name) = words.next() else {
            return;
        };

        // :42 で42行目へ、:$ で末尾へ。
        let line = match name {
            "$" => Some(usize::MAX),
            _ => name.parse::<usize>().ok(),
        };

        if let Some(line) = line {
            self.move_to(line.saturating_sub(1));
            return;
        }

        if name == "set" {
            // :set number nonumber のように並べられる。
            let options: Vec<&str> = words.collect();
            if options.is_empty() {
                self.message =
                    "no setting specified".to_string();
                return;
            }
            for option in options {
                self.set_option(option);
            }
            return;
        }

        let argument = words.next();

        match name {
            "w" => {
                self.write(argument);
            }
            "wt" => {
                self.write_tree(argument);
            }
            "wq" | "x" => {
                if self.write(argument) {
                    self.quit = true;
                }
            }
            "q" => {
                if self.modified {
                    self.message =
                        "unsaved changes. \
                         Use :q! to discard"
                            .to_string();
                } else {
                    self.quit = true;
                }
            }
            "q!" => self.quit = true,
            "e" => {
                self.edit(argument);
            }
            "e!" => {
                self.modified = false;
                self.edit(argument);
            }
            _ => {
                self.message =
                    format!("unknown command: {}", name)
            }
        }
    }

    /// :set の項目を1つ処理する。
    fn set_option(&mut self, option: &str) {
        match option {
            "number" | "nu" => self.number = true,
            "nonumber" | "nonu" => self.number = false,
            "number!" | "nu!" => {
                self.number = !self.number
            }
            _ => {
                self.message = format!(
                    "unknown setting: {}",
                    option
                )
            }
        }
    }

    /// ファイルを読む。読めたらtrueを返す。
    fn edit(&mut self, argument: Option<&str>) -> bool {
        if self.modified {
            self.message =
                "unsaved changes. \
                 Use :e! to discard"
                    .to_string();
            return false;
        }

        if let Some(name) = argument {
            self.path = Some(PathBuf::from(name));
        }

        let Some(path) = self.path.clone() else {
            self.message =
                "no file name".to_string();
            return false;
        };

        self.load(&path)
    }

    pub(super) fn load(&mut self, path: &Path) -> bool {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) => {
                self.message =
                    format!("cannot read: {}", error);
                return false;
            }
        };

        let reading = match reader::read(&text) {
            Ok(reading) => reading,
            Err(error) => {
                self.message = format!(
                    "cannot read {}: {}",
                    path.display(),
                    error
                );
                return false;
            }
        };

        self.nodes = nodes_from(&reading.data);
        self.cursor = 0;
        self.cursor_col = 0;
        self.modified = false;
        // ソース表示中に別ファイルを開くと食い違うので、
        // 木の表示に戻す。
        self.source_mode = false;
        // ファイルが入れ替わるので履歴は捨てる。
        self.undo.clear();
        self.redo.clear();
        self.held = None;

        self.message = if reading.hoisted > 0 {
            format!(
                "read {}. Moved {} comment(s) found \
                 inside expressions to the start \
                 of the line",
                path.display(),
                reading.hoisted
            )
        } else {
            format!("read {}", path.display())
        };

        true
    }

    /// ファイルに書く。書けたらtrueを返す。
    fn write(&mut self, argument: Option<&str>) -> bool {
        if let Some(name) = argument {
            self.path = Some(PathBuf::from(name));
        }

        let Some(path) = self.path.clone() else {
            self.message =
                "no file name".to_string();
            return false;
        };

        // ソース表示中はdepth:0のフラットな行なので、
        // to_scheme()に通さずそのまま連結する。
        let mut text = if self.source_mode {
            self.nodes
                .iter()
                .map(|node| node.text.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            self.to_scheme()
        };
        text.push('\n');

        match fs::write(&path, text) {
            Ok(()) => {
                self.modified = false;
                self.message = format!(
                    "wrote {}",
                    path.display()
                );
                true
            }
            Err(error) => {
                self.message =
                    format!("cannot write: {}", error);
                false
            }
        }
    }

    /// 木の見た目（罫線＋テキスト）をtxtとして書き出す。
    ///
    /// 表示モードに関わらず常に木として計算する。
    /// ソース表示中は、今の内容を読み直して木を組み
    /// 立て直すだけで、実際のモードは変えない。
    fn write_tree(&mut self, argument: Option<&str>) -> bool {
        if let Some(name) = argument {
            self.tree_path = Some(PathBuf::from(name));
        }

        let Some(path) = self.tree_path.clone() else {
            self.message = "no file name".to_string();
            return false;
        };

        let lines = if self.source_mode {
            let text = self
                .nodes
                .iter()
                .map(|node| node.text.as_str())
                .collect::<Vec<_>>()
                .join("\n");

            let reading = match reader::read(&text) {
                Ok(reading) => reading,
                Err(error) => {
                    self.message = format!(
                        "cannot parse back into a tree: {}",
                        error
                    );
                    return false;
                }
            };

            render_tree_lines(&nodes_from(&reading.data))
        } else {
            self.tree_lines()
        };

        let mut text = lines.join("\n");
        text.push('\n');

        match fs::write(&path, text) {
            Ok(()) => {
                self.message =
                    format!("wrote {}", path.display());
                true
            }
            Err(error) => {
                self.message =
                    format!("cannot write: {}", error);
                false
            }
        }
    }
}
