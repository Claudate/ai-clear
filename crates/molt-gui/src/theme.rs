//! 主题：语义色、字号、圆角、深浅色跟随系统。
//!
//! 颜色只在这里定义一次。界面代码里不出现字面颜色值——那样每加一个界面
//! 就要再决定一次「这块该多灰」，最后必然不一致。
//!
//! 设计取向（对着 docs/research.md 的结论）：目标用户里有一半不懂电脑，
//! 他们判断一个清理工具是否可信，靠的是「配色克制、文字清楚、主按钮只有一个」。
//! 所以整套界面只有一种强调色（蓝，代表「干净、可以做」），
//! 红色只留给不可撤销的操作，受保护项一律用中性灰。
//!
//! 底色与中性色都带一点冷调（不是纯灰），和强调色同族：界面因此读起来是
//! 一整块蓝调而不是「蓝按钮贴在灰纸上」——这正是「以蓝色为基调」的意思。
//!
//! egui 0.37 起，浅色与深色各持一份 `Style`。所以这里的做法是：
//! 两套样式都装上一遍，之后由 egui 根据系统外观自己挑。

use eframe::egui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Light,
    Dark,
}

/// 一套界面用色。全部按语义命名，不按颜色命名。
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    /// 窗口背景
    pub bg: egui::Color32,
    /// 侧栏背景：比窗口底深半档的同族灰蓝。
    ///
    /// 侧栏不能用 `surface`（白卡色）——一条白色竖带贴在白内容区旁，
    /// 侧栏与内容的边界就消失了；也不能用 `surface_alt` 那样的凹槽色，
    /// 它读起来像「被按下去」。macOS 侧栏的观感是「比内容区略深的同一块底」。
    pub sidebar_bg: egui::Color32,
    /// 行与行之间的细分隔线：比 `border` 再淡一档。
    /// 清单改用访达式行列表后，行间靠这条 1 像素的线而不是卡片描边分彼此——
    /// 用 `border` 画几十条会整页发灰。
    pub row_separator: egui::Color32,
    /// 卡片 / 分组的背景
    pub surface: egui::Color32,
    /// 更弱一层的背景（表头、输入框、分段控件底槽）
    pub surface_alt: egui::Color32,
    /// 悬停时的浅底
    pub surface_hover: egui::Color32,
    /// 主要文字
    pub text: egui::Color32,
    /// 次要文字
    pub text_dim: egui::Color32,
    /// 三级文字（脚注、单位）
    pub text_faint: egui::Color32,
    /// 分隔线
    pub border: egui::Color32,
    /// 需要更明确边界的描边（输入框、卡片强调边）
    pub border_strong: egui::Color32,
    /// 可回收 / 主按钮 / 选中态。整个界面只有这一种强调色。
    pub reclaimable: egui::Color32,
    /// 强调色的浅底，用于选中项与图标底
    pub accent_soft: egui::Color32,
    /// 有代价
    pub costly: egui::Color32,
    /// 有代价的浅底
    pub costly_soft: egui::Color32,
    /// 受保护——刻意用灰而不是红
    pub protected: egui::Color32,
    /// 磁盘饼图里「已用、但清不掉」那一段。
    ///
    /// 它既不是「能清」（蓝）也不是「你的记录」（浅灰），必须是第三档中性色，
    /// 否则饼图会被读成「一大块都是能清的」或「一大块都是我的文件」。
    pub used_bar: egui::Color32,
    /// 危险 / 失败 / 空间告急。只在「必须马上看」的地方用，
    /// 不能拿它当普通强调——满屏红会让真正危险的那条也被忽略。
    pub danger: egui::Color32,
    /// 危险的浅底
    pub danger_soft: egui::Color32,
    /// 字号缩放比例。1.0 为标准档。
    ///
    /// 放在调色板里而不是单独传给每个函数：界面里每一处字号都要乘它，
    /// 一旦漏传一处，那一处就不会跟着放大——而这种漏最难发现，
    /// 因为平时（缩放为 1）看起来完全正常。
    pub scale: f32,
}

/// 圆角尺度。三条就够：卡片、控件、小标记。
pub const RADIUS_CARD: u8 = 14;
pub const RADIUS_CONTROL: u8 = 9;
pub const RADIUS_PILL: u8 = 6;

impl Palette {
    pub fn light() -> Self {
        Self {
            bg: egui::Color32::from_rgb(244, 247, 252),
            sidebar_bg: egui::Color32::from_rgb(233, 239, 247),
            row_separator: egui::Color32::from_rgb(212, 221, 234),
            surface: egui::Color32::from_rgb(255, 255, 255),
            surface_alt: egui::Color32::from_rgb(236, 242, 250),
            surface_hover: egui::Color32::from_rgb(242, 246, 252),
            text: egui::Color32::from_rgb(18, 27, 40),
            text_dim: egui::Color32::from_rgb(92, 106, 128),
            text_faint: egui::Color32::from_rgb(140, 153, 173),
            border: egui::Color32::from_rgb(219, 228, 241),
            border_strong: egui::Color32::from_rgb(194, 208, 227),
            reclaimable: egui::Color32::from_rgb(21, 101, 216),
            accent_soft: egui::Color32::from_rgb(228, 238, 253),
            costly: egui::Color32::from_rgb(158, 92, 16),
            costly_soft: egui::Color32::from_rgb(250, 241, 228),
            protected: egui::Color32::from_rgb(139, 146, 152),
            used_bar: egui::Color32::from_rgb(126, 143, 168),
            danger: egui::Color32::from_rgb(179, 54, 46),
            danger_soft: egui::Color32::from_rgb(251, 236, 234),
            scale: 1.0,
        }
    }

    pub fn dark() -> Self {
        Self {
            bg: egui::Color32::from_rgb(13, 19, 28),
            sidebar_bg: egui::Color32::from_rgb(18, 26, 38),
            row_separator: egui::Color32::from_rgb(40, 53, 70),
            surface: egui::Color32::from_rgb(22, 30, 42),
            surface_alt: egui::Color32::from_rgb(30, 41, 56),
            surface_hover: egui::Color32::from_rgb(26, 36, 48),
            text: egui::Color32::from_rgb(234, 241, 250),
            text_dim: egui::Color32::from_rgb(154, 168, 190),
            text_faint: egui::Color32::from_rgb(116, 132, 155),
            border: egui::Color32::from_rgb(37, 49, 65),
            border_strong: egui::Color32::from_rgb(54, 69, 90),
            reclaimable: egui::Color32::from_rgb(101, 163, 255),
            accent_soft: egui::Color32::from_rgb(23, 41, 66),
            costly: egui::Color32::from_rgb(224, 168, 100),
            costly_soft: egui::Color32::from_rgb(45, 36, 23),
            protected: egui::Color32::from_rgb(140, 148, 156),
            used_bar: egui::Color32::from_rgb(110, 128, 153),
            danger: egui::Color32::from_rgb(240, 124, 114),
            danger_soft: egui::Color32::from_rgb(49, 30, 28),
            scale: 1.0,
        }
    }

    pub fn of(mode: Mode) -> Self {
        match mode {
            Mode::Light => Self::light(),
            Mode::Dark => Self::dark(),
        }
    }

    pub fn with_scale(mut self, scale: f32) -> Self {
        self.scale = scale;
        self
    }

    /// 把基准字号换成当前档位的实际字号。
    ///
    /// 界面里写字一律走这里（`.size(p.sz(13.0))`），不要直接写 `.size(13.0)`：
    /// `RichText::size` 会完全覆盖 `style.text_styles`，写死的字号
    /// 无论怎么调样式都不会跟着变——这正是放大档位一开始「按钮文字变了、界面没变大」的原因。
    pub fn sz(&self, base: f32) -> f32 {
        base * self.scale
    }

    /// 铺在强调色上的文字色。
    ///
    /// 浅色模式强调色是饱和蓝，配近白字；深色模式强调色是浅蓝，配深字。
    /// 两处都由 `bg` 天然满足（它正好是强调色的反衬），所以直接复用它，
    /// 不再引入一个容易写反的新字段。
    pub fn on_accent(&self) -> egui::Color32 {
        self.bg
    }

    /// 某个判定对应的颜色。界面里取色一律走这里，不再自己 match。
    pub fn verdict(&self, v: &molt_core::Verdict) -> egui::Color32 {
        match v {
            molt_core::Verdict::Reclaimable => self.reclaimable,
            molt_core::Verdict::Costly { .. } => self.costly,
            molt_core::Verdict::Protected { .. } => self.protected,
        }
    }

    /// 某个判定对应的浅底，用于标签与分组底色。
    pub fn verdict_soft(&self, v: &molt_core::Verdict) -> egui::Color32 {
        match v {
            molt_core::Verdict::Reclaimable => self.accent_soft,
            molt_core::Verdict::Costly { .. } => self.costly_soft,
            molt_core::Verdict::Protected { .. } => self.surface_alt,
        }
    }
}

pub struct Theme {
    pub mode: Mode,
    pub palette: Palette,
    /// 字号缩放。固定走 [`Theme::DEFAULT_SCALE`]（大字号）。
    pub scale: f32,
    /// 是否跟随系统外观。
    ///
    /// 真正的窗口恒为 true；验证工具置 false 才能把深浅两档都拍下来——
    /// 否则 `sync_system` 每一帧都会把配色拉回系统当前那一档，
    /// 深色截图变成浅色，而图上完全看不出这是错的。
    pub follow_system: bool,
}

impl Theme {
    /// 唯一档位：**大字号**。
    ///
    /// 字号切换（标准字号/放大字号）已按用户要求移除，界面固定用这一档。
    /// 这款工具的门槛恰恰是「一眼看清这一项是什么、删了会怎样」，
    /// 所以字号直接落在大字号，不给用户切小的入口。
    pub const DEFAULT_SCALE: f32 = 1.4;

    /// 安装主题并返回当前状态。两套样式都装，之后跟随系统切换。
    pub fn install(ctx: &egui::Context) -> Self {
        let mut t = Self {
            mode: current_mode(ctx),
            palette: Palette::light(),
            scale: Self::DEFAULT_SCALE,
            follow_system: true,
        };
        t.palette = Palette::of(t.mode).with_scale(t.scale);
        t.apply_all(ctx);

        // 跟随系统外观，而不是锁死其中一个
        ctx.set_theme(egui::ThemePreference::System);
        t
    }

    fn apply_all(&self, ctx: &egui::Context) {
        let scale = self.scale;
        for (theme, palette) in [
            (egui::Theme::Light, Palette::light()),
            (egui::Theme::Dark, Palette::dark()),
        ] {
            let palette = palette.with_scale(scale);
            ctx.style_mut_of(theme, |style| apply(style, palette, theme));
        }
    }

    /// 跟随系统深浅色。每帧调用——用户在系统设置里切换时窗口要立即跟上。
    pub fn sync_system(&mut self, ctx: &egui::Context) {
        if !self.follow_system {
            return;
        }
        let mode = current_mode(ctx);
        if mode != self.mode {
            self.mode = mode;
            self.palette = Palette::of(mode).with_scale(self.scale);
            self.apply_all(ctx);
        }
    }
}

/// 按一份 `Theme` 重新安装两套样式。
///
/// 公开它是因为验证工具需要在不动系统外观的前提下渲染指定档位；
/// 截图的档位与窗口里能切出来的档位是同一套常量，不会对不上。
pub fn reinstall(ctx: &egui::Context, t: &Theme) {
    for (theme, palette) in [
        (egui::Theme::Light, Palette::light()),
        (egui::Theme::Dark, Palette::dark()),
    ] {
        let palette = palette.with_scale(t.scale);
        ctx.style_mut_of(theme, |style| apply(style, palette, theme));
    }
}

fn apply(style: &mut egui::Style, p: Palette, theme: egui::Theme) {
    style.visuals = match theme {
        egui::Theme::Light => egui::Visuals::light(),
        egui::Theme::Dark => egui::Visuals::dark(),
    };
    style.visuals.panel_fill = p.bg;
    style.visuals.window_fill = p.surface;
    style.visuals.extreme_bg_color = p.surface_alt;
    style.visuals.faint_bg_color = p.surface_alt;
    style.visuals.window_stroke = egui::Stroke::new(1.0, p.border);
    style.visuals.window_corner_radius = egui::CornerRadius::same(RADIUS_CARD);
    style.visuals.menu_corner_radius = egui::CornerRadius::same(RADIUS_CONTROL);
    // 注意：**不要**设 override_text_color。
    // 它会无条件盖掉所有文字颜色，于是 egui 按主题换 Visuals 的机制就失效了——
    // 表现是深色模式下正文变成暗色压在暗背景上，几乎不可见。
    // 文字色已经在上面通过 `Visuals::light()/dark()` 带上了，需要强调时用 RichText 显式上色。
    style.visuals.widgets.noninteractive.bg_stroke.color = p.border;
    style.visuals.widgets.noninteractive.bg_fill = p.surface;
    style.visuals.widgets.inactive.bg_stroke.color = p.border_strong;
    style.visuals.widgets.inactive.weak_bg_fill = p.surface;
    style.visuals.widgets.hovered.weak_bg_fill = p.surface_hover;
    style.visuals.widgets.hovered.bg_stroke.color = p.border_strong;
    style.visuals.widgets.active.weak_bg_fill = p.surface_alt;
    for w in [
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
        &mut style.visuals.widgets.open,
    ] {
        w.corner_radius = egui::CornerRadius::same(RADIUS_CONTROL);
    }
    style.visuals.selection.bg_fill = p.accent_soft;
    style.visuals.selection.stroke.color = p.reclaimable;
    style.visuals.hyperlink_color = p.reclaimable;

    // 文本样式每次从默认值重建再乘缩放，而不是在现有值上继续乘——
    // `style_mut_of` 改的是持久样式，累乘会让来回切两档之后字号越变越大、回不去。
    style.text_styles = egui::Style::default().text_styles;
    for font in style.text_styles.values_mut() {
        font.size *= p.scale;
    }

    // 6 点是这套界面的基本单位。间距比旧版略松：卡片之间要能呼吸，
    // 挤在一起会让「这一块和那一块是两件事」看不出来。
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(14.0, 7.0);
    // 可点目标不小于 30 点：macOS 控件高度约在这个量级，按钮太扁会难点中。
    // 放大字号时同步抬高，否则字比按钮还高，会被裁掉。
    style.spacing.interact_size.y = 30.0 * p.scale;
    style.spacing.scroll.bar_width = 8.0;
    style.spacing.scroll.floating = true;
}

fn current_mode(ctx: &egui::Context) -> Mode {
    match ctx.theme() {
        egui::Theme::Dark => Mode::Dark,
        egui::Theme::Light => Mode::Light,
    }
}

use crate::icons;

/// 把系统中文字体装进 egui。
///
/// 装两处：`Proportional`（正文）和 `Monospace`（等宽）。
/// 等宽那处也要装——路径、编号这些用等宽显示时若缺中文字形，
/// 目录名里的汉字会变成方块。
pub fn install_fonts(ctx: &egui::Context) {
    let Some((_name, bytes)) = crate::fonts::load_cjk_font() else {
        return;
    };

    let mut fonts = egui::FontDefinitions::default();
    const KEY: &str = "system-cjk";

    fonts.font_data.insert(
        KEY.to_owned(),
        std::sync::Arc::new(egui::FontData::from_owned(bytes)),
    );

    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        let list = fonts.families.entry(family).or_default();
        // 放在末尾：拉丁字符优先用 egui 自带字体（字重更匀），汉字回落到系统字体。
        list.push(KEY.to_owned());
    }

    // 图标字体单独一个家族，**不**混进正文：按字符码在正文里回落到图标字体，
    // 会把正文里万一出现的私用区字符画成图标，那是很难查的意外。只给
    // [`crate::icons`] 用，它拿到的每个字符都确定是图标。
    fonts.font_data.insert(
        icons::FONT_KEY.to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(icons::FONT_BYTES)),
    );
    fonts
        .families
        .insert(icons::family(), vec![icons::FONT_KEY.to_owned()]);

    ctx.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palettes_are_distinct_per_mode() {
        let l = Palette::light();
        let d = Palette::dark();
        assert_ne!(l.bg, d.bg);
        assert_ne!(l.text, d.text);
        // 浅色底色应该比文字亮，深色反过来
        assert!(l.bg.r() > l.text.r());
        assert!(d.bg.r() < d.text.r());
    }

    #[test]
    fn muted_protected_is_gray_not_red() {
        // 受保护用灰不用红：红色会让人以为「危险但可删」
        for p in [Palette::light(), Palette::dark()] {
            let c = p.protected;
            let spread = c.r().abs_diff(c.g()).max(c.g().abs_diff(c.b()));
            assert!(spread <= 12, "受保护色应接近中性灰，实际 {c:?}");
        }
    }

    #[test]
    fn reclaimable_and_costly_are_semantically_opposed() {
        // 可回收偏冷（蓝）、有代价偏暖（r 最大）——冷暖对立，一眼可分。
        // 不再用「绿 vs 红」那一对：界面整体是蓝调，绿色强调会在蓝底上
        // 变成第二个完全不同的色系，而蓝与暖橙在同一套冷色底上仍然分明。
        for p in [Palette::light(), Palette::dark()] {
            assert!(p.reclaimable.b() > p.reclaimable.r());
            assert!(p.costly.r() > p.costly.g());
        }
    }

    #[test]
    fn accent_text_always_contrasts_with_its_fill() {
        // 主按钮的文字色取 bg，必须与强调底形成明显明暗差；
        // 否则浅色/深色某一侧会出现「白字压白底」这种点不动的按钮。
        for p in [Palette::light(), Palette::dark()] {
            let on = p.on_accent();
            let diff = (p.reclaimable.r() as i32 - on.r() as i32).abs()
                + (p.reclaimable.g() as i32 - on.g() as i32).abs()
                + (p.reclaimable.b() as i32 - on.b() as i32).abs();
            assert!(diff > 180, "主按钮文字与底色对比不足：{diff}");
        }
    }

    #[test]
    fn verdict_colors_come_from_the_palette() {
        use molt_core::{CostKind, RebuildCost, Verdict};
        let p = Palette::light();
        assert_eq!(p.verdict(&Verdict::Reclaimable), p.reclaimable);
        assert_eq!(
            p.verdict(&Verdict::Costly {
                rebuild: RebuildCost::new(CostKind::Recompile, "重编")
            }),
            p.costly
        );
    }

    #[test]
    fn sz_scales_every_font_size() {
        // 界面里所有字号都走 Palette::sz。这条守的是「有一处漏乘缩放」——
        // 那种漏平时看不出来，只有在放大档位下才会露出一行没变大的字。
        assert_eq!(Palette::light().sz(13.0), 13.0);
        let big = Palette::light().with_scale(1.4);
        assert!((big.sz(10.0) - 14.0).abs() < 0.001);
        // 缩放跟着档位走，颜色不跟着变
        assert_eq!(big.text, Palette::light().text);
    }
}
