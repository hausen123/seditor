# S-editor

S-expression editor for scheme.
A TUI editor that lets you edit S-expressions as a tree instead of parentheses.

```
cargo install --path .
sedit test/test.scm
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

Vim-like: starts in Normal mode, `i` / `a` enter Insert mode, `Esc` returns to Normal mode for movement and editing. Most keys below take a count (`3j`, `5dd`, ...).

| Key | Action |
|---|---|
| `F2` | toggle tree / Scheme source view |
| `>>` `<<`, `Tab` `Shift-Tab` (Normal) | indent subtree down / up |
| `i` `a` `I` `A` | insert: here / after / line start / line end |
| `o` `O` | new node below / above, and insert |
| `Esc` | back to Normal mode |
| `h` `j` `k` `l` | left / down / up / right |
| `0` `$` `Home` `End` | line start / end |
| `gg` `G` `M` | first / last / middle-of-screen node |
| `x` `Delete` | delete a character (yanks / doesn't) |
| `dd` `D` | delete node / cut subtree |
| `yy` `Y` | yank node / subtree |
| `p` `P` | paste after / before |
| `u` `Ctrl-r` | undo / redo |
| `Ctrl-f` `Ctrl-b` `Ctrl-d` `Ctrl-u` | scroll page / half-page |
| `/` `?` `n` `N` | search / repeat |
| `*` `#` | search for the node under the cursor |
| `v` `V` | visual mode: char-wise / line-wise |

In Visual mode, movement extends the selection; `y` `d` `c` yank / delete / change it, `>` `<` indent it (line-wise), `:` starts a command scoped to the selection, `Esc` cancels it.

In Insert mode, `Tab` / `Shift-Tab` change the depth of just the current line, instead of the whole subtree.

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
