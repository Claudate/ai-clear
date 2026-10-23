#!/bin/zsh
# 启动「蜕」窗口。写成脚本是因为直接把二进制路径敲进终端时，
# IDE 终端的回显会把它截断成 `arget/release/...` 而启动失败。
cd "$(dirname "$0")" || exit 1
exec ./target/release/molt-gui
