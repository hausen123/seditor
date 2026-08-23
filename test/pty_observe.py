#!/usr/bin/env python3
"""
sedit の実バイナリを疑似端末(pty)で起動し、キーを1つずつ
送りながら、その都度の画面を「反転表示されているセルを
[x] で囲んだテキスト」として出力するデバッグ用スクリプト。

## 目的

cargo test のユニットテストは tree_display() が返す
Line/Span の中身を検証するだけで、実際の端末に何が描画
されるか（本当に反転表示されているか、キー入力が正しく
届くか）までは保証しない。カーソルや選択範囲の表示に関する
不具合は、実機の描画結果を目で見て確認しないと確定できない
ことがある（実例: ビジュアルモードで矢印キーが無反応だった
不具合、文字単位選択がノードを跨いだときの反転表示崩れ、
などはこの方法で発見・検証した）。

## 使い方

    python3 test/pty_observe.py [--bin PATH] [--file PATH]
        [--cols N] [--rows N] [--rows-shown N] KEY [KEY ...]

KEY は1つずつ順番に送られ、送るたびに現在の画面を
スナップショットして出力する。特殊キーは以下のように書く。

    ESC          -> \\x1b
    ENTER        -> \\n
    TAB          -> \\t
    それ以外の文字列はそのままキー入力として送られる
    （例: "v" は v キー、"3j" は 3 と j を続けて送る）

例（このファイルを直接実行した場合の例と同じ）:

    python3 test/pty_observe.py --file test/test.scm \\
        ":22" ENTER v j j j j j

## 実装上の注意点（このスクリプトが対処していること）

- 疑似端末は既定でサイズが0x0で、そのままだと何も描画され
  ない。fcntl.ioctl(TIOCSWINSZ) で明示的にサイズを設定する
  必要がある。
- ratatui は差分描画なので、状態が変わらない操作の後は出力
  が空になることがある。「出力が無い＝無反応」ではなく、
  累積した画面状態で判断すること。
- pyte.Screen.display() は一部の文字（◦ など）で
  wcwidth 周りのバグにより例外を出すことがあるため、
  screen.buffer[y][x].data / .reverse を直接読む。
- 受信済みバイト列は、任意の過去のバイト位置に巻き戻して
  再パースするとエスケープシーケンスやマルチバイト文字の
  途中で切れて壊れることがある。スナップショットは、その
  都度受信済みのバイト列をそのまま pyte へ feed した直後の
  状態をコピーして保存する（今回はスクリーンをその場で
  文字列化するだけなのでコピーの必要すらない）。

## 依存

    pip install pyte
"""

import argparse
import fcntl
import os
import pty
import select
import struct
import sys
import termios
import threading
import time

import pyte

SPECIAL_KEYS = {
    "ESC": "\x1b",
    "ENTER": "\n",
    "TAB": "\t",
    "BACKTAB": "\x08",
    "BACKSPACE": "\x7f",
    "DELETE": "\x04",
}


def resolve_key(token: str) -> str:
    return SPECIAL_KEYS.get(token, token)


def row_text(screen: pyte.Screen, y: int, width: int) -> str:
    row = screen.buffer[y]
    out = []
    for x in range(width):
        cell = row[x]
        d = cell.data if cell.data else " "
        out.append(f"[{d}]" if cell.reverse else d)
    return "".join(out).rstrip()


def main() -> None:
    parser = argparse.ArgumentParser(
        description=(
            "sedit を疑似端末で動かし、キーを送るたびの"
            "画面（反転セルは[x]表記）を出力する"
        )
    )
    parser.add_argument(
        "--bin",
        default=os.path.join(
            os.path.dirname(__file__),
            "..",
            "target",
            "debug",
            "sedit",
        ),
        help="sedit バイナリのパス（既定: target/debug/sedit）",
    )
    parser.add_argument(
        "--file", default=None, help="開くファイル（省略可）"
    )
    parser.add_argument(
        "--cols", type=int, default=100, help="端末の幅"
    )
    parser.add_argument(
        "--rows", type=int, default=40, help="端末の高さ"
    )
    parser.add_argument(
        "--rows-shown",
        type=int,
        default=None,
        help="出力する行数（既定: --rowsから2引いた値）",
    )
    parser.add_argument(
        "--delay",
        type=float,
        default=0.2,
        help="キー送信後に待つ秒数（描画が追いつくまでの猶予）",
    )
    parser.add_argument(
        "keys",
        nargs="+",
        help="送るキーの列。ESC/ENTER/TABなどは特殊キー名で",
    )
    args = parser.parse_args()

    rows_shown = args.rows_shown or (args.rows - 2)

    argv = [os.path.abspath(args.bin)]
    if args.file:
        argv.append(args.file)

    pid, fd = pty.fork()
    if pid == 0:
        os.execv(argv[0], argv)
        os._exit(1)

    winsize = struct.pack(
        "HHHH", args.rows, args.cols, 0, 0
    )
    fcntl.ioctl(fd, termios.TIOCSWINSZ, winsize)

    screen = pyte.Screen(args.cols, args.rows)
    stream = pyte.ByteStream(screen)
    lock = threading.Lock()
    stop = False

    def reader() -> None:
        while not stop:
            ready, _, _ = select.select([fd], [], [], 0.05)
            if ready:
                try:
                    chunk = os.read(fd, 65536)
                except OSError:
                    break
                if not chunk:
                    break
                with lock:
                    stream.feed(chunk)

    thread = threading.Thread(target=reader, daemon=True)
    thread.start()

    time.sleep(0.5)

    def snapshot(label: str) -> None:
        with lock:
            rows = [
                row_text(screen, y, args.cols)
                for y in range(rows_shown)
            ]
        print(f"=== {label} ===")
        print("\n".join(rows))
        print()

    snapshot("start")
    for token in args.keys:
        key = resolve_key(token)
        time.sleep(args.delay)
        os.write(fd, key.encode())
        time.sleep(args.delay)
        snapshot(token)

    os.write(fd, b"\x1b")
    time.sleep(0.1)
    os.write(fd, b":q!\n")
    time.sleep(0.3)

    stop = True
    try:
        os.kill(pid, 9)
    except ProcessLookupError:
        pass


if __name__ == "__main__":
    main()
