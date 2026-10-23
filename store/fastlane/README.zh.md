# fastlane 配置

这个目录是 `store/` 物料的**产物**，不是来源：`metadata/` 与 `screenshots/`
由 `store/tools/sync_fastlane.py` 从 `store/` 单向同步过来，不要在这里手改
（下次同步会覆盖）。

fastlane 每次运行会重写这个 README。本文件由同步脚本写入，
若被覆盖，重跑 `python3 store/tools/sync_fastlane.py` 即可恢复。

## 四条 lane

```bash
cd store/fastlane
FASTLANE_SKIP_UPDATE_CHECK=1 fastlane ios <lane>   # 跳过更新检查，输出才看得清
```

| lane | 做什么 |
|---|---|
| `preflight` | 只检查，什么都不做。缺什么直接列出来 |
| `metadata` | 同步商店页（文案 + 截图），不动二进制 |
| `upload` | 传二进制（转调 `store/tools/upload.sh`） |
| `release` | 传二进制 → 等处理 → 同步商店页 |

## 目录约定

```
metadata/
├── zh-Hans/  en-US/
│   ├── name.txt  subtitle.txt  keywords.txt
│   ├── promotional_text.txt  description.txt  release_notes.txt
│   ├── support_url.txt  privacy_url.txt  marketing_url.txt
└── review_information/
    ├── notes.txt                store/review-notes.md 的英文正文
    └── first_name / last_name / email_address / phone_number.txt  （需自己填）
screenshots/
└── zh-Hans/  en-US/  *.png      文件名编号决定商店里的排列顺序
```

## 两个 URL 必须填真地址

`support_url.txt` 与 `privacy_url.txt` 现在还是 `example.com` 占位值，
`preflight` 会把它们判为未通过。Apple 会实际抓取这两个地址，抓不到就提交不了。
填法：直接改 `store/fastlane/metadata/<locale>/` 下的两个文件，
或改 `store/tools/sync_fastlane.py` 里的 `PLACEHOLDERS` 再同步一次。

## 审核联系人

`metadata/review_information/` 下的姓名、邮箱、电话是空的占位文件，
必须填真人真联系方式。审核被拒时 Apple 先发邮件再在 Resolution Center 留言，
联系方式不实是常见的无谓延误。
