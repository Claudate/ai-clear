# 上架物料

App Store 需要的图片与文字都在这里，可直接对应 App Store Connect 的各个输入框。
所有图都由脚本生成，改完界面重跑一次就能同步，不需要手工重做。

```
store/
├── icon-1024.png              商店图标 1024×1024，RGB，无 alpha
├── icon.iconset/              打进 .app 的十档图标（保留透明）
├── icon.icns                  由 iconset 打包，放 Resources/AppIcon.icns
├── screenshots/
│   ├── zh-Hans/01..06.png     2880×1800，无 alpha
│   └── en-US/01..06.png       同上
├── metadata/
│   ├── zh-Hans/               name / subtitle / keywords / promotional_text
│   │                          / description / release_notes
│   └── en-US/                 同上
├── privacy-policy.zh-Hans.md  需发布到公网后把 URL 填进 App Store Connect
├── privacy-policy.en-US.md
├── app-privacy-answers.md     「App 隐私」问卷逐题该选什么
├── review-notes.md            「审核备注」可直接粘贴的英文说明
├── make_icon.py               图标生成（品牌色取自 theme.rs）
├── make_screenshots.py        截图合成（底图来自 shots 的真实渲染）
├── check_limits.py            字段长度校验，超限退出 1
│
├── release.sh                 一条命令走完：出图 → 打包 → 同步 → 上传
├── build_app.sh               编译 + 组装 .app + 签名 + 出 .pkg / .zip
├── bundle/
│   ├── Info.plist             bundle 模板（版本号在打包时填入）
│   ├── molt-mas.entitlements  沙箱 + 家目录只读（上架用）
│   └── molt-dev.entitlements  非沙箱（Developer ID 直供用）
├── tools/
│   ├── sign_pkg.sh            .pkg 签名 + 钥匙串 partition list 修复
│   ├── upload.sh              上传到 App Store Connect（iTMSTransporter）
│   └── sync_fastlane.py       把本目录映射成 fastlane 的目录结构
└── fastlane/
    ├── Fastfile               preflight / metadata / upload / release 四条 lane
    ├── metadata/              同步产物，勿手改
    └── screenshots/           同步产物，勿手改
```

## 一键上架

```bash
./store/release.sh --dry-run   # 出图、打包、同步、跑到前置检查为止
./store/release.sh             # 同上，并通过后真的上传
```

四个步骤分开跑也可以：

```bash
./store/build_app.sh mas       # .app + .pkg（沙箱签名）
./store/tools/sign_pkg.sh      # pkg 签名（首次需先 --setup）
python3 store/tools/sync_fastlane.py
./store/tools/upload.sh --check # 只查不传
```

### 上传需要的凭据

凭据一律走环境变量，仓库里不放任何密钥：

```bash
export ASC_API_KEY_ID=XXXXXXXXXX
export ASC_API_ISSUER_ID=xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx
export ASC_TEAM_ID=HA7KN8S95K
# 把 AuthKey_<KEY_ID>.p8 放到 ~/.appstoreconnect/private_keys/
```

App Store Connect → 用户和访问 → 集成 → App Store Connect API，新建密钥后
下载 .p8（只能下载一次）。Issuer ID 在同一页顶部。

### 两个只有你能做的一次性动作

1. **放开钥匙串里私钥的 partition list。** 跑一次
   `./store/tools/sign_pkg.sh --setup`，会提示输入登录钥匙串的密码。
   不做的后果：`.app` 能签、`.pkg` 签不了，报 `errSecInternalComponent(-2070)`
   或 `EPERM 100001`——看着像证书坏了，其实只是 ACL 没放开。
   这一步必须交互输入密码，所以只能由你本人跑一次。
2. **审核联系人。** `store/fastlane/metadata/review_information/` 下的
   姓名、邮箱、电话要填真人真联系方式。

## 重新生成

```bash
cargo run -p molt-gui --bin shots -- target/shots   # 真实界面底图
python3 store/make_icon.py                           # 图标
iconutil -c icns store/icon.iconset -o store/icon.icns  # 打进 .app 用的 icns
python3 store/make_screenshots.py                    # 截图
python3 store/check_limits.py                        # 字段校验
```

## 提交前逐项核对

| 项目 | 要求 | 当前状态 |
|---|---|---|
| 图标 | 1024×1024，无 alpha，不要自己加圆角 | 已满足（四角为白底，非透明） |
| 截图 | 16:10，可选 2880×1800 等；无 alpha；1–10 张 | 中英各 6 张，2880×1800 |
| 名称 | ≤ 30 字符 | 中文 8，英文 23 |
| 副标题 | ≤ 30 字符 | 中文 14，英文 28 |
| 关键词 | ≤ 100 **字节** | 中文 97，英文 86 |
| 宣传文本 | ≤ 170 字符 | 中文 101，英文 168 |
| 描述 | ≤ 4000 字符 | 中文 1111，英文 3205 |
| 版本说明 | ≤ 4000 字符 | 中文 215，英文 516 |
| 隐私政策 URL | 公网可访问，不能登录才看得到 | **待发布** |
| 支持 URL | 必填 | **待定** |
| 营销 URL | 选填 | 可不填 |

## 还需要你自己准备的

1. **隐私政策发布地址。** 把 `privacy-policy.*.md` 发布到你的网站上（GitHub Pages
   也行），填进 App Store Connect。它会实际抓取，抓不到无法提交。
2. **支持邮箱。** 隐私政策里写的是 `support@molt.app`，换成你真实能收信的地址，
   并与「支持 URL」页面上的一致。
3. **应用图标是否要换成设计稿。** 现在这版是从品牌色推出来的几何图形
   （青绿底 + 三条自下而上收窄的横杠）。要换只需改 `make_icon.py` 里的常量。
4. **Bundle ID 与 SKU。** 建议 `app.molt.mac` / `molt-mac-001`，首次创建后不可改。
5. **年龄分级问卷。** 全部选「无」即可：不含暴力、赌博、成人内容、用户生成内容。

## 一个前提

按 2026-10-22 核过的 Guideline 2.4.5，当前形态（自动发现 `~/.claude`、
`~/Library/Application Support/Cursor` 等沙箱外路径 + launchd 定时任务）**直接提交
MAS 会被拒**，卡点是沙箱而不是文案或图片。这份物料对两条路都有用：

- 走 MAS：需要先做沙箱版（security-scoped bookmarks + `SMAppService`），
  这份 listing 与截图可以直接复用；
- 走 Developer ID 公证直供：商店图片用不上，但图标、隐私政策、官网文案、
  审核备注里的说明（尤其是「为什么需要完全磁盘访问」）仍然要写，已一并备好。
