//! 卷容量：这块盘总共多大、已用多少、还能用多少。
//!
//! 为什么用 `statvfs` 而不是 `df` / `diskutil`：界面每次刷新都要读一次，
//! fork 一个进程既慢，又在某些受限环境下拿不到输出——那正是「读不到就留空」的
//! 典型成因。`statvfs` 与 Finder、`df` 读的是同一组内核计数，
//! 实测与 `diskutil info /` 的 Container Total/Free Space 完全一致。
//!
//! 注意 macOS 的 APFS 容器语义：系统卷与数据卷共享同一个容器，
//! 所以在 `/` 与 `/System/Volumes/Data` 上读到的 total/available 是同一个数。
//! 这正是用户关心的那个「还剩多少」，不是单个卷的配额。

use std::path::Path;

/// 一次容量读数。三个数字都来自同一次 `statvfs`，彼此自洽。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskUsage {
    /// 容器总容量
    pub total: u64,
    /// 已用 = total - 空闲块（含系统保留块，与 Finder 的「已用」同口径）
    pub used: u64,
    /// 当前用户可用。它比 `total - used` 小，差额是系统保留空间，
    /// 所以「还剩多少」必须用这个字段，不能自己减。
    pub available: u64,
}

impl DiskUsage {
    /// 已用占比，0.0 ~ 1.0。
    pub fn used_ratio(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        (self.used as f64 / self.total as f64) as f32
    }

    /// 以「已用 + 可用」为分母的占比。
    ///
    /// 画进度条时用这个而不是 `used_ratio`：总量里有一部分是系统保留空间，
    /// 既不算已用也不能被用户使用，画进去会让条永远填不满、看起来像数据错了。
    pub fn bar_ratio(&self) -> f32 {
        let denom = self.used.saturating_add(self.available);
        if denom == 0 {
            return 0.0;
        }
        (self.used as f64 / denom as f64) as f32
    }

    /// 可用空间的占比，用于「还剩多少」的阈值提示。
    pub fn available_ratio(&self) -> f32 {
        let denom = self.used.saturating_add(self.available);
        if denom == 0 {
            return 0.0;
        }
        (self.available as f64 / denom as f64) as f32
    }
}

/// 读某个路径所在卷的容量。
///
/// 传入家目录而不是 `/` 是有意的：用户关心的是「我的盘还剩多少」，
/// 而家目录在数据卷上；将来若把家目录放到外置卷，这里也会跟着读对。
pub fn read(path: impl AsRef<Path>) -> Result<DiskUsage, String> {
    let path = path.as_ref();
    let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| format!("路径含非法字节：{}", path.display()))?;

    // SAFETY: `statvfs` 只写满这个结构体，指针在调用期间有效。
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c.as_ptr(), &mut st) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        return Err(format!("读取 {} 的容量失败：{err}", path.display()));
    }

    // 优先 f_frsize（真实块大小）。某些文件系统只填 f_bsize，
    // 取 0 时会让三个数字一起变成 0——那种「显示 0 B」比报错更难排查。
    let frsize = if st.f_frsize > 0 {
        st.f_frsize as u64
    } else {
        st.f_bsize as u64
    };
    if frsize == 0 {
        return Err("文件系统没有报告有效的块大小".to_string());
    }

    let total = (st.f_blocks as u64).saturating_mul(frsize);
    let free = (st.f_bfree as u64).saturating_mul(frsize);
    let available = (st.f_bavail as u64).saturating_mul(frsize);

    if total == 0 {
        return Err("文件系统报告的容量为 0".to_string());
    }

    Ok(DiskUsage {
        total,
        used: total.saturating_sub(free),
        available,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_real_volume() {
        let d = read(std::env::temp_dir()).expect("临时目录所在卷必须可读");
        // 只要这台机器上跑着测试，盘就不可能是 0 字节
        assert!(d.total > 0, "总容量不应为 0");
        assert!(d.available <= d.total, "可用不应超过总量");
        assert!(d.used <= d.total, "已用不应超过总量");
        // 已用 + 空闲 = 总量，可用只是其中一部分
        assert!(d.available <= d.total - d.used, "可用不应超过空闲块");
    }

    #[test]
    fn ratios_stay_in_range() {
        let d = DiskUsage {
            total: 1000,
            used: 800,
            available: 150,
        };
        assert!((d.used_ratio() - 0.8).abs() < 1e-6);
        // 进度条以「已用 + 可用」为分母：系统保留的 50 不参与
        assert!((d.bar_ratio() - 800.0 / 950.0).abs() < 1e-6);
        assert!(d.bar_ratio() <= 1.0);
        assert!(d.available_ratio() > 0.0);
    }

    #[test]
    fn zero_total_does_not_divide_by_zero() {
        let d = DiskUsage {
            total: 0,
            used: 0,
            available: 0,
        };
        assert_eq!(d.used_ratio(), 0.0);
        assert_eq!(d.bar_ratio(), 0.0);
    }

    #[test]
    fn missing_path_reports_an_error_instead_of_zeroes() {
        // 路径不存在时必须报错，不能返回一堆 0 —— 那会被界面画成「盘空了」
        let err = read("/no/such/path/for/molt").unwrap_err();
        assert!(err.contains("读取"), "实际：{err}");
    }
}
