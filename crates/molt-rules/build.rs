//! 规则库的编译期校验。
//!
//! 为什么放在 build.rs 而不是运行时：一条写错的规则在生产里的表现是
//! 「静默不命中」——用户以为清干净了其实没有，或者更糟，规则写宽了删多了。
//! 在编译期炸掉是唯一能保证「发出去的二进制里没有坏规则」的做法。
//!
//! 这里只做**语法与结构**校验（能否解析、id 是否重复、有代价的项是否写了 note）。
//! 语义校验（模式是否合理、靶子是否存在）在运行时做，因为它依赖真实文件系统。

use std::path::Path;

fn main() {
    let rules_dir = Path::new("../../rules");
    println!("cargo:rerun-if-changed=../../rules");

    let Ok(entries) = std::fs::read_dir(rules_dir) else {
        println!(
            "cargo:warning=找不到规则目录 {}，本次构建跳过规则校验",
            rules_dir.display()
        );
        return;
    };

    let mut total = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let text = std::fs::read_to_string(&path).expect("读取规则文件失败");

        // 用占位家目录校验结构：这里只验语法，不验路径是否存在
        match molt_core::RuleSet::from_toml(&name, &text, "/Users/placeholder") {
            Ok(rs) => total += rs.len(),
            Err(e) => panic!("\n规则文件校验失败：{e}\n"),
        }
    }

    if total == 0 {
        panic!("规则目录里一个规则都没有，构建中止");
    }
    println!("cargo:warning=规则库校验通过，共 {total} 条");
}
