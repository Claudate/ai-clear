# Molt · 蜕

> Tidy up. That's it. / 蜕好，就好。

给 **AI 编程工具**用的磁盘清理器。清的是 Codex、Claude Code、Cursor、VS Code 这些工具留下的缓存，以及 Rust / Go / Node 的构建产物——不是系统垃圾。

## 为什么不直接删

因为删错一次就够了。这个工具的三条硬约束：

1. **扫描只读。** `molt scan` 不写任何东西。
2. **默认不删，只移走。** `clean` 不加 `--apply` 只演练；加了之后内容进隔离区，7 天内 `restore` 随时放回去。
   要立刻释放空间可以显式选「直接清理」——它不经过隔离区，不可恢复，所以总是要再确认一次。
3. **受保护的东西进不去。** 密钥、配置、会话历史由代码内嵌的硬禁区挡住，规则库改错也拦得住。

## 三种判定

| 判定 | 含义 | 默认勾选 |
|---|---|---|
| **可回收** | 删了自动重建，无感 | 是 |
| **有代价** | 可重建，但要重编 / 重下 / 重装 | 否，并给出代价说明 |
| **受保护** | 你的记录、密钥、配置 | 永不 |

第三类用灰色而不是红色——红色会让人以为「危险但可删」，灰色才是「这条路不存在」。

## 用

```bash
cargo build --release
./target/release/molt scan          # 只读，看清单
./target/release/molt clean         # 演练，显示会做什么
./target/release/molt clean --apply # 真执行（交互式确认后移入隔离区）
./target/release/molt restore <id>  # 放回去
./target/release/molt history       # 待还原列表
```

`scan --json` 输出机器可读结果，`jq` 能直接吃。

## 窗口

```bash
cargo run -p molt-gui
```

三页：概览（先回答「该不该动手」）、清单（逐条勾选，标注删了会怎样）、隔离区（查看 / 还原 / 彻底删除）、历史（操作时间线）。

空、加载中、出错、几百条条目这四种状态都有各自的界面：
扫描失败时不会说「很干净」，读不到隔离区时不会画成「空列表」。

出一份全部状态的可视核对图（不需要显示器或录屏权限）：

```bash
cargo run -p molt-gui --bin shots -- target/shots
```

它跑的是同一份界面代码，因此图上的东西就是用户会看到的东西。

窗口**不做任何判定**——扫描、判定、清理全部调 `molt-core` / `molt-scan` / `molt-act` 里那一套。
同一个逻辑只有一个入口，界面上看到的和命令行敲出来的就必然一致。

清单页有两个并列的动作，位置与颜色都不同：

- **移入隔离区**（绿色实心，主操作）：可还原，但文件只是被移走，**可用空间不变**。
- **直接清理…**（红色描边，次要位置）：立即释放空间，**不可恢复**。

点任一个都要再确认一次。永久删除的确认框会写明「永久删除、不可恢复」并列出将要删除的对象与数量。

隔离区是固定入口（顶部标签带数量），在里面可以逐条或批量还原，也可以彻底删除。

## 无人值守清理

```bash
molt auto                  # 演练：看看会清什么
molt auto --apply          # 真清（无人确认，只动下面这一类）
molt schedule install      # 每周日 03:00 自动跑 auto --apply
molt schedule status
molt schedule uninstall
```

**自动清理只碰规则里写了 `auto = true` 的项**，且必须同时是可回收。构建产物
（`target`、`node_modules`、各类下载缓存）一律不标——它们能重建，但一次无人值守的清理
会让第二天开机就面对十分钟重编。规则加载期就拒绝「有代价的项标 auto」，写错会编译失败。

审计表里每条记录带 `source`：`manual` 是有人在终端敲了 yes，`auto` 是定时任务自己做的。
事后翻历史能分清这两类。

## AI 深度分析

```bash
cargo run -p molt-gui     # 窗口里点「AI 分析」
cargo run -p molt-ai --example pipeline        # 七段流程端到端（临时目录，不碰真实家目录）
cargo run -p molt-ai --example model_roundtrip  # 验证「配置里选的模型确实生效」
```

七段流程：扫描 → 分析 → 方案 → 预览确认 → 执行 → 撤销 → 复查清理。
**默认不删只移**：执行只走隔离区或压缩归档，没有「直接删除」这条路径；
永久清理是复查页里用户单独确认的动作。

配置在 `~/.config/molt/ai.toml`（首次启动自动生成模板，也可在窗口里改并保存）：

```toml
active_provider = "deepseek"
active_model = "deepseek-chat"
allow_content_sample = false

[[provider]]
id = "deepseek"
kind = "open_ai_compatible"     # 或 "anthropic"
base_url = "https://api.deepseek.com/v1"
api_key_env = "DEEPSEEK_API_KEY"  # 优先读环境变量
# api_key = "sk-..."             # 也可以直接写，但会被提示风险
official = false

[[provider.model]]
id = "deepseek-chat"
label = "DeepSeek Chat"
context_tokens = 64000
json_mode = true
```

模型列表**完全来自配置**，代码里没有任何写死的模型 id——`active_model` 留空时
程序只会说「还没选模型」，不会偷偷用第一个。官方（Claude / OpenAI）与第三方
（DeepSeek / 本地 Ollama / 任何 OpenAI 兼容中转）只是同一套协议的两个分支。

删除判断默认用 **jev**（TypeSafe 的 System One 模型）。它是一个只看状态、只回
带类型判断的模型，不是聊天模型，所以走单独的 `system_one` 协议：

```toml
[[provider]]
id = "typesafe"
kind = "system_one"
base_url = "https://api.typesafe.ai"
api_key_env = "TYPESAFE_API_KEY"

[[provider.model]]
id = "jev"
label = "jev"
context_tokens = 32000
json_mode = false
```

没配 `TYPESAFE_API_KEY` 时删除判断会明确报「服务 typesafe 还没有可用密钥」，
不会悄悄换用别的模型。你已经在 `active_model` 里选了别的模型时，删除判断按你的
选择走——jev 只是「还没选」时的默认值。

**发给模型的东西是受控的**：默认只发路径、大小、类型、时间这些元数据；
文件内容默认不读，打开后也同时受「小于 256 KB」「每文件 4 KB」「总量 32 KB」
「最多 24 个文件」四道上限约束，且命中密钥指纹的文件整段拒发。

**AI 只给建议，判定仍在本地的确定性规则里**：模型提到的路径必须能在本次扫描结果
里原样找到，受保护项、家目录之外的系统路径、照片/合同/财务类文件一律被拒；
目标路径由本地算，释放空间按实测重算，不用 AI 报的数字。被拒的每一条都会连同
原因一起显示，不会静默消失。

AI 不可用时（没配模型、没密钥、网络失败、模型回了非 JSON）页面会说明原因并给出
对应的下一步，本地扫描与清单页的手动清理完全不受影响。

## 结构

```
molt-core     判定逻辑，纯函数，不碰文件系统
molt-safety   四道闸：硬禁区 / 密钥指纹 / 进程占用 / 符号链接逃逸
molt-scan     只读遍历，按 inode 去重，量逻辑大小与真实占用
molt-act      唯一能写盘的层：隔离区 / 压缩归档 / 审计 / 还原
molt-rules    规则库（TOML，编译期校验）
molt-ai       AI 分析：配置 / 摘要 / 结构化解析 / 方案核对（不写盘）
molt-cli      命令行
molt-gui      窗口（eframe/egui，不做判定，只调上面几层）
```

依赖是单向的：判定只在 `molt-core` 里有一份，CLI 与将来的 GUI 都调它。`molt-act` 是唯一有写权限的 crate，扫描层只有 `O_RDONLY`——类型上就不可能误删。

## 规则

内置在 `rules/*.toml`，随二进制发布。`~/.config/molt/rules/` 下的同名文件会整体替换内置版本。

一条规则的形态：

```toml
[[rule]]
id = "cursor-history"
patterns = ["~/Library/Application Support/Cursor/User/History/**"]
what = "Cursor 的文件历史快照（AI 改得越勤涨得越快）"
verdict = { type = "costly", cost = "lose_record", note = "会丢失这些文件在本地的历史版本" }
```

有代价的规则必须写 `note`——没写会导致编译失败。这是有意的：一条说不出代价的规则，等于没想清楚。

有代价的规则还可以写一个闲置阈值：

```toml
[[rule]]
id = "rust-target"
patterns = ["**/target/**"]
what = "Rust 编译产物"
stale_days = 30          # 这条命中的路径超过 30 天没动过，就标「建议清理」
verdict = { type = "costly", cost = "recompile", note = "全量重编 4-20 分钟" }
```

`stale_days` 只能写在有代价的规则上，写在可回收或受保护的规则上会导致编译失败：
可回收的项本来就该清，受保护的永远不动，给它们标一个天数只会让规则读起来像
「久了就可以删」。

**它是建议，不是第四种判定。** 判定的三态一个都没变，命中的项仍然是有代价项、
仍然不默认勾选、仍然不参与 `molt auto`。阈值只影响两件事：`scan` 里多一行
「建议清理 · 62 天未使用」，以及清单页多一个「建议清理」筛选（在这一档里可以一键全选本页）。
读不到访问时间的项不会被标——没数据不等于很久没用。

目前只有 `rust-target`、`rust-target-misc`（30 天）与 `xcode-deriveddata`（30 天）带阈值。

内置规则覆盖 45 条，其中这几类占据空间最多，判定上区别对待：

| 项目 | 判定 | 理由 |
|---|---|---|
| Xcode `DerivedData` | 有代价（重新编译）· 30 天未用标「建议清理」 | 能重建，但一次全量重编要十分钟起 |
| Xcode `DeviceSupport` | 有代价（重新下载） | 下次连设备调试时重新提取 |
| Xcode `Archives` | 受保护 | 发布归档与 dSYM 无法重建 |
| 模拟器 `Caches` | 有代价 | 启动模拟器时重建 |
| 模拟器 `Devices` | 受保护 | 设备里的 app 数据是工作成果 |
| Docker `Docker.raw` | 受保护 | 全部镜像与容器，交给 `docker system prune` |
| Rust `target` | 有代价（重新编译）· 30 天未用标「建议清理」 | 一次全量重编 4-20 分钟，是最占地方的一类 |
| AI 工具的内核缓存 | 可回收 + `auto` | 删了自动重建，不联网、不掉登录态 |

Docker 与模拟器设备数据判为「受保护」而不是「可清理」，是因为它们不是缓存——
工具负责告诉你它有多大，删不删由那个软件自己决定。

## 设计取舍

**不做内存优化。** macOS 的内存由内核管，第三方「内存释放」收益趋近于零。磁盘才是「每次都要清」的原因。

**AI 不参与判定。** 删除是不可逆动作，不能由概率决定。AI 只能解释「这是什么」，判定永远由确定性规则 + 哨兵产生。

**按叶子级命中，不按父目录。** `~/Library/Application Support/Code/` 下面既有可再生的 `Code Cache`，也有你的 `User/History`。按父目录判定一次就会连带删掉历史版本。

## 许可

MIT
