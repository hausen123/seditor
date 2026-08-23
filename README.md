# S-editor

S-expression editor for scheme.
A TUI editor that lets you edit S-expressions as a tree instead of parentheses.

```
cargo build --release
sedit foo.scm
```

## How the tree is written

A node's text is the head of the list; its children are the remaining elements.

```
define              (define (square x)
├── square            (* x x))
│   └── x
└── *
    ├── x
    └── x
```

A list with no leading symbol keeps its text empty. It's shown as `◦` on screen.

```
cond                (cond ((< x 2) 1)
├── ◦                     ((> x 2) -1))
│   ├── <
│   │   ├── x
│   │   └── 2
│   └── 1
└── ◦
    ├── >
    │   ├── x
    │   └── 2
    └── -1
```

`'` `` ` `` `,` `,@` `#` `#u8` `#;` `#0=` are treated as prefix markers. Since the contents of a quote are unevaluated data, the first element isn't used as a heading — the elements are simply listed as children as-is. Only an empty list gets an `◦` inserted.

```
'x           '(a b)         '(1 2 3)       '()
'x           '              '              '
(1 node)     ├── a          ├── 1          └── ◦
              └── b          ├── 2
                              └── 3
```

A nested list with no marker still uses `◦`, as before.

```
'(a (b c) d)

'
├── a
├── ◦
│   ├── b
│   └── c
└── d
```

## Keys

Starts in Normal mode. `Esc` returns to it.
In Insert mode, `Tab` / `Shift-Tab` change the depth of just that line.
In Normal mode, `Tab` / `Shift-Tab` / `>>` / `<<` move the cursor's subtree together with its descendants.
In Insert mode, `Home` / `End` move to the start/end of the line. Pressing `Enter` in the middle of text splits the node there; `Backspace` at the start of the line merges into the previous node, and `Delete` at the end of the line merges into the next node. `Backspace` and `Delete` don't yank even when they remove a whole node.

On supported terminals, a paste (bracketed paste) is treated as a single edit. `Tab` or control characters inside the pasted text are inserted as literal characters rather than being interpreted as key input. A paste in Normal mode is also never executed as a command sequence — it's inserted as text at the cursor position.

| Key | Action |
|---|---|
| `F2` | Toggle between tree and Scheme view. The source view can also be edited. The cursor lands at the corresponding position, and the screen scrolls to center it |
| `h` `j` `k` `l` | Left / down / up / right. `j` `k` move by row, `h` `l` move within the text |
| `0` `$` `Home` `End` | Start / end of line |
| `gg` `G` | First / last node |
| `M` | The node in the middle of what's currently visible on screen |
| `i` `a` `I` `A` | Enter Insert mode: in place / one right / start of line / end of line |
| `o` `O` | Create an empty node below / above and enter Insert mode |
| `x` | Delete one character. Yanked |
| `Delete` | Delete one character. Not yanked |
| `dd` | Delete the node and promote its children by one level. The deleted content is yanked |
| `yy` `Y` | Yank the node / subtree |
| `D` | Cut the subtree |
| `p` `P` | Paste as the next sibling / before the cursor |
| `Tab` `Shift-Tab` (Normal) | Move the subtree one level down / up |
| `>>` `<<` | Move the subtree one level down / up |
| `u` `Ctrl-r` | Undo / redo |
| `Ctrl-f` `Ctrl-b` | Scroll one screen forward / back |
| `Ctrl-d` `Ctrl-u` | Scroll half a screen forward / back |
| `PageDown` `PageUp` | Scroll one screen forward / back |
| `/` `?` | Search forward / backward. A regular expression (Rust regex syntax) matched against a node's text |
| `n` `N` | Repeat the last search in the same / opposite direction |
| `*` `#` | Search forward / backward using the cursor's node text as the pattern |
| `g&` | Reapply the last `:s` substitution (its token list and flags) to all nodes, using the last search pattern |
| `v` `V` | Enter Visual mode: char-wise / line-wise. Pressing the other one just switches the kind; pressing the same key again exits |

While in Visual mode, `h` `j` `k` `l` `0` `$` `gg` `G` (with counts) extend the selection, and the keys below act on it. Char-wise (`v`) may cross nodes via `j` `k` `gg` `G`; when it does, it behaves like vim's char-wise visual spanning multiple lines: the node where the selection starts is selected from that point to its end, the node where it ends is selected from its start up to that point, and any node fully enclosed in between is selected in full.

| Key | Action (char-wise / line-wise) |
|---|---|
| `y` | Yank the selection. `register` is always a list of nodes, shaped like a line-wise `Nyy` (for char-wise, only the two boundary nodes — where the selection doesn't align with node boundaries — become copies with their text trimmed to the selected part; any fully enclosed node in between is untouched). The cursor returns to the start of the selection |
| `d` `x` | Delete the selection and yank it. A node fully contained in the selection is removed entirely (same as `dd`; its children are promoted by one level). A node only partially selected (char-wise) just has that part of its text removed and remains (the two boundary nodes are never merged into one) |
| `c` | Same deletion as `d`, then enters Insert mode (line-wise, or when a whole node was removed: an empty node is created and Insert mode starts there. If a partially-remaining node exists from a char-wise selection, you continue typing right after what's left of it) |
| `>` `<` | Line-wise only. Moves each subtree in the selection one level down / up. Selecting a parent along with its children doesn't move the children twice |
| `:` | Enters Command mode with `'<,'>` pre-filled on the command line (continue with something like `:'<,'>s/.../.../`) |
| `Esc` | Just cancels the selection; node contents are unchanged |

Pasting a register that came from Visual mode with `p` / `P` always pastes at the node level (inserted as new sibling nodes). This is also true when a char-wise yank/delete stayed within a single node — it is never inserted inline at that spot.

`u` undoes an entire Insert-mode session at once. Everything typed from `i` to `Esc` disappears together.

`h` `j` `k` `l` `x` `dd` `yy` `p` `P` `G` can be prefixed with a count. `yy` operates per node, so to duplicate a subtree, count its rows and use something like `3yy`.

Inside tmux, the default prefix key is `Ctrl-b`, so `Ctrl-b` never reaches the app. Use `PageUp` instead, or change tmux's prefix key.

## Commands

| Command | Action |
|---|---|
| `:w` | Save |
| `:w {file}` | Save under the given name, and use that name from now on |
| `:e` | Reload |
| `:e {file}` | Open |
| `:q` | Quit |
| `:q!` `:e!` | Discard unsaved changes |
| `:wq` `:x` | Save and quit |
| `:42` | Go to node 42. `:$` goes to the last one |
| `:set number` | Show line numbers. Can be abbreviated `nu` |
| `:set nonumber` | Hide line numbers. Toggle with `number!` |
| `:[range]s/{pattern}/{replacement}/[flags]` | Equivalent to vim's `:substitute`. Replaces text within nodes |
