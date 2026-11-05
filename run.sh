#!/bin/zsh
# 启动「净灵」窗口。
#
# 写成脚本的一个原因是：直接把二进制路径敲进终端时，IDE 终端的回显会
# 把它截断（曾经变成 `arget/release/...`、`./target //debug/...`），
# 于是启动失败。脚本里的路径不经过那层回显。
#
# 传 `debug` 跑 target/debug，其余情况跑 release（用户看到的版本）。
cd "$(dirname "$0")" || exit 1
BIN_DIR=release
[ "$1" = "debug" ] && BIN_DIR=debug
[ -x "./target/$BIN_DIR/molt-gui" ] || { echo "没找到 target/$BIN_DIR/molt-gui，先 cargo build"; exit 1; }
exec "./target/$BIN_DIR/molt-gui"
