//! 密钥指纹哨兵：删除前扫一眼「这玩意儿里面是不是藏着 key」。
//!
//! 为什么需要它：硬禁区认的是路径和扩展名，而真实项目里最危险的东西恰恰长着
//! 一张垃圾脸——`config.yaml.bak.20260923`、`prod.json`、`settings` 无扩展名。
//! cpa 那个目录里有 11 个 `config.yaml.bak.*`，每个都含真实上游 key，
//! 靠扩展名完全拦不住。
//!
//! 成本控制：只看前 64 KB，只看候选集，不递归扫全盘。

use std::path::Path;

/// 单次读取上限。密钥几乎总在文件开头（或整个文件就很小），
/// 读满 64 KB 还没命中就认为不含指纹——这是有意的误报率取舍。
pub const MAX_SCAN_BYTES: u64 = 64 * 1024;

/// 超过这个大小的文件不扫：配置文件与密钥文件不会这么大，
/// 扫大文件只是浪费 IO，还会拖慢整次扫描。
pub const MAX_FILE_SIZE: u64 = 1024 * 1024;

/// 值得内容扫描的扩展名。
const SUSPICIOUS_EXTENSIONS: &[&str] = &[
    "env",
    "yaml",
    "yml",
    "json",
    "toml",
    "ini",
    "conf",
    "cfg",
    "properties",
    "pem",
    "key",
    "txt",
];

pub struct Pattern {
    pub name: &'static str,
    /// 前缀签名。用「以什么开头 + 后续长度」判定，而不是完整正则——
    /// 这样不必为一条规则引入 regex 依赖，也更容易测。
    pub prefix: &'static str,
    /// 签名之后还需要至少多少个「像 token 的字符」
    pub min_body: usize,
}

/// 指纹库。新增 key 格式只需要在这里加一行，不用改判定逻辑。
pub const PATTERNS: &[Pattern] = &[
    Pattern {
        name: "openai_key",
        prefix: "sk-",
        min_body: 20,
    },
    Pattern {
        name: "sk_tr_key",
        prefix: "sk_tr_",
        min_body: 20,
    },
    Pattern {
        name: "aws_access_key",
        prefix: "AKIA",
        min_body: 16,
    },
    Pattern {
        name: "github_token",
        prefix: "ghp_",
        min_body: 36,
    },
    Pattern {
        name: "github_pat",
        prefix: "github_pat_",
        min_body: 30,
    },
    Pattern {
        name: "slack_token",
        prefix: "xoxb-",
        min_body: 10,
    },
    Pattern {
        name: "slack_token",
        prefix: "xoxp-",
        min_body: 10,
    },
    Pattern {
        name: "anthropic_key",
        prefix: "sk-ant-",
        min_body: 20,
    },
    Pattern {
        name: "private_key_block",
        prefix: "-----BEGIN ",
        min_body: 5,
    },
];

/// 命中结果。**只带模式名和文件路径，绝不带命中的内容**——
/// 这条约束写在类型里：`Hit` 没有存放原文的字段，想泄漏也做不到。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub pattern: &'static str,
    pub file: std::path::PathBuf,
}

/// 这个文件值不值得读内容。纯文件名判断，不碰 IO。
pub fn is_worth_scanning(path: &Path, size: u64) -> bool {
    if size == 0 || size > MAX_FILE_SIZE {
        return false;
    }
    let name = match path.file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => return false,
    };
    // 无扩展名的文件（settings、credentials、.netrc）恰恰经常是密钥文件
    // 注意 `config.yaml.bak.20260923` 的 extension() 只会返回 `20260923`，
    // 所以必须再看一眼文件名里有没有 `.bak`——那正是 cpa 里那批备份的形状。
    let lower = name.to_ascii_lowercase();
    if lower.contains(".bak") || lower.contains("backup") {
        return true;
    }
    match path.extension().and_then(|e| e.to_str()) {
        None => true,
        Some(ext) => {
            let ext = ext.to_ascii_lowercase();
            SUSPICIOUS_EXTENSIONS.contains(&ext.as_str())
        }
    }
}

/// 在一段文本里找密钥指纹。返回模式名（不含命中内容）。
pub fn scan_text(text: &str) -> Option<&'static str> {
    for pat in PATTERNS {
        if contains_token(text, pat.prefix, pat.min_body) {
            return Some(pat.name);
        }
    }
    None
}

/// 找到 `prefix` 之后，检查紧跟的 `min_body` 个字符是否都是 token 字符。
///
/// 这一步是必要的：单看 `sk-` 前缀会把 `sk-learn`、`sk-image` 这类正常文字判成密钥，
/// 误报一次用户就再也不会信这个工具。
fn contains_token(text: &str, prefix: &str, min_body: usize) -> bool {
    let mut from = 0usize;
    while let Some(idx) = text[from..].find(prefix) {
        let start = from + idx + prefix.len();
        if start >= text.len() {
            // 前缀在文件末尾，后面没内容，不可能是完整密钥
            return false;
        }
        // PEM 头是 `-----BEGIN RSA PRIVATE KEY-----`，正文是字母和空格，需要单独放宽
        if prefix == "-----BEGIN " {
            let rest = &text[start..];
            if rest.contains("PRIVATE KEY-----") {
                return true;
            }
        } else {
            let body: String = text[start..]
                .chars()
                .take(min_body)
                .take_while(|c| is_token_char(*c))
                .collect();
            if body.chars().count() >= min_body {
                return true;
            }
        }
        from = start;
    }
    false
}

/// 允许出现在 token 里的字符。覆盖 base64url、hex、以及 JWT 用的 `-`/`_`/`.`。
fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')
}

/// 读取文件开头并检查。IO 失败一律当作「没命中」——
/// 读不到不代表危险，但也不该让整次扫描崩掉。
pub fn check_file(path: &Path) -> Option<Hit> {
    use std::io::Read;

    let meta = std::fs::metadata(path).ok()?;
    if !is_worth_scanning(path, meta.len()) {
        return None;
    }
    let mut buf = vec![0u8; MAX_SCAN_BYTES.min(meta.len()) as usize];
    let mut f = std::fs::File::open(path).ok()?;
    let n = f.read(&mut buf).ok()?;
    buf.truncate(n);
    // 非 UTF-8 一律用有损转换：密钥都是 ASCII，丢几个字符不影响判定
    let text = String::from_utf8_lossy(&buf);
    scan_text(&text).map(|pattern| Hit {
        pattern,
        file: path.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_openai_style_key_is_detected() {
        assert_eq!(
            scan_text("OPENAI_API_KEY=sk-proj-AbCdEf0123456789AbCdEf0123456789"),
            Some("openai_key")
        );
    }

    #[test]
    fn cpa_style_key_is_detected() {
        // cpa 的 public/ 里就是这种格式
        assert_eq!(
            scan_text("key: sk_tr_9f3aBcDeFgHiJkLmNoPqRsTuV"),
            Some("sk_tr_key")
        );
    }

    #[test]
    fn ordinary_word_starting_with_sk_is_not_a_key() {
        // 「sk-learn」「sk-image」这类正常字符串不能误报
        assert_eq!(scan_text("import sk-learn as x"), None);
        assert_eq!(scan_text("sk-image-2"), None);
    }

    #[test]
    fn pem_private_key_header_is_detected() {
        assert_eq!(
            scan_text("-----BEGIN RSA PRIVATE KEY-----\nMIIE..."),
            Some("private_key_block")
        );
        assert_eq!(
            scan_text("-----BEGIN OPENSSH PRIVATE KEY-----"),
            Some("private_key_block")
        );
    }

    #[test]
    fn public_key_block_is_not_flagged_as_private() {
        assert_eq!(scan_text("-----BEGIN PUBLIC KEY-----\nMIIB..."), None);
    }

    #[test]
    fn bare_prefix_at_eof_does_not_panic() {
        assert_eq!(scan_text("sk-"), None);
        assert_eq!(scan_text(""), None);
    }

    #[test]
    fn aws_and_github_tokens_are_detected() {
        assert_eq!(scan_text("AKIAIOSFODNN7EXAMPLE"), Some("aws_access_key"));
        assert_eq!(
            scan_text("ghp_1234567890abcdefghijklmnopqrstuvwxyz"),
            Some("github_token")
        );
    }

    #[test]
    fn extensionless_files_are_worth_scanning() {
        assert!(is_worth_scanning(Path::new("/x/credentials"), 100));
        assert!(is_worth_scanning(Path::new("/x/.netrc"), 100));
    }

    #[test]
    fn backup_of_a_config_is_worth_scanning() {
        // 这是硬禁区拦不住的那一类：config.yaml.bak.20260923
        assert!(is_worth_scanning(
            Path::new("/x/config.yaml.bak.20260923"),
            200_000
        ));
    }

    #[test]
    fn huge_files_and_binaries_are_skipped() {
        assert!(!is_worth_scanning(Path::new("/x/model.bin"), 100));
        assert!(!is_worth_scanning(
            Path::new("/x/big.json"),
            MAX_FILE_SIZE + 1
        ));
        assert!(!is_worth_scanning(Path::new("/x/empty.env"), 0));
    }

    #[test]
    fn hit_type_carries_no_secret_material() {
        let h = Hit {
            pattern: "openai_key",
            file: std::path::PathBuf::from("/x/config.yaml"),
        };
        // 编译期就能过的检查：Hit 只有两个字段，没有存放原文的地方
        let _ = (h.pattern, h.file);
    }
}
