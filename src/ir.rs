//! 中间表示（IR）。
//!
//! IR 是「已经换算到画布 EMU 坐标系、但还没绑定任何 OOXML 概念」的一层描述。
//! 存在的意义是把 SVG 前端和 PPTX 后端解耦：换后端只需重写 `emit.rs`，
//! 而且转换逻辑可以不生成 pptx 就直接断言。

use crate::geom::Rgb;

/// 一页幻灯片。
#[derive(Debug, Clone, Default)]
pub struct Page {
    pub width_emu: i64,
    pub height_emu: i64,
    /// 背景色；`None` 表示不画背景（透明）。
    pub background: Option<Rgb>,
    /// 按 z 序排列的内容（先画的在底层）。
    pub items: Vec<Item>,
}

/// 页面上的一个元素。
#[derive(Debug, Clone)]
pub enum Item {
    /// 矢量形状（`a:custGeom` 路径）。
    Shape(ShapeItem),
    /// 内嵌位图。
    Image(ImageItem),
    /// 无法矢量表达、已局部光栅化的内容。
    Raster(RasterItem),
    /// 可编辑的 PPT 文本框（`a:txBody`）。
    Text(TextItem),
}

/// EMU 坐标系下的矩形框。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

/// `a:custGeom` 的绘制指令，坐标相对所属形状的左上角，单位为 EMU。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathCmd {
    MoveTo(i64, i64),
    LineTo(i64, i64),
    CubicTo(i64, i64, i64, i64, i64, i64),
    Close,
}

/// 填充或描边的颜料。
#[derive(Debug, Clone, PartialEq)]
pub enum Paint {
    None,
    Solid(Rgb),
    /// 线性渐变。角度单位是 SVG 语义的「度」（顺时针为正，y 轴向下）。
    LinearGradient {
        angle_deg: f64,
        stops: Vec<Stop>,
    },
}

/// 渐变停靠点，`pos` 取 0.0~1.0。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stop {
    pub pos: f64,
    pub color: Rgb,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapKind {
    Butt,
    Round,
    Square,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum JoinKind {
    Miter,
    Round,
    Bevel,
}

/// DrawingML 只支持 11 种预设虚线，SVG 的任意 `stroke-dasharray` 只能就近映射。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashKind {
    Dot,
    Dash,
    LargeDash,
    DashDot,
    LargeDashDot,
    LargeDashDotDot,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stroke {
    pub paint: Paint,
    pub width_emu: i64,
    pub cap: CapKind,
    pub join: JoinKind,
    pub dash: Option<DashKind>,
}

#[derive(Debug, Clone)]
pub struct ShapeItem {
    pub name: String,
    pub frame: Frame,
    /// 相对 `frame` 左上角的路径指令。
    pub commands: Vec<PathCmd>,
    pub fill: Option<Paint>,
    pub stroke: Option<Stroke>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Gif,
}

#[derive(Debug, Clone)]
pub struct ImageItem {
    pub name: String,
    pub frame: Frame,
    pub data: Vec<u8>,
    pub format: ImageFormat,
}

#[derive(Debug, Clone)]
pub struct RasterItem {
    pub name: String,
    pub frame: Frame,
    /// 已经编码好的 PNG 字节。
    pub png: Vec<u8>,
}

/// 文本的水平对齐。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextHAlign {
    Left,
    Center,
    Right,
}

/// 一个可编辑文本框（当前只处理单行）。
#[derive(Debug, Clone)]
pub struct TextItem {
    pub name: String,
    pub frame: Frame,
    pub align: TextHAlign,
    /// 同一行内的若干 run（颜色/字体不同会被切成多个 run）。
    pub runs: Vec<TextRunSpec>,
}

/// 文本框内的一段同样式文字。
#[derive(Debug, Clone)]
pub struct TextRunSpec {
    pub text: String,
    pub font_family: Option<String>,
    /// 字号，百分之一磅（DrawingML `<a:rPr sz="..">`）。
    pub size_100ths_pt: i32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub color: Rgb,
}

/// 一条降级/告警记录。
///
/// 转换过程中的任何「信息丢失」都必须产生一条诊断，不允许静默吞掉。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// 稳定的机器可读代号，便于测试断言与 CI 守门。
    pub code: &'static str,
    pub detail: String,
}

impl Diagnostic {
    pub fn new(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

/// 一次转换的诊断集合。
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub diagnostics: Vec<Diagnostic>,
}

impl Report {
    pub fn push(&mut self, code: &'static str, detail: impl Into<String>) {
        self.diagnostics.push(Diagnostic::new(code, detail));
    }

    pub fn extend(&mut self, other: Report) {
        self.diagnostics.extend(other.diagnostics);
    }

    pub fn is_empty(&self) -> bool {
        self.diagnostics.is_empty()
    }

    /// 按代号聚合计数，用于 CLI 摘要输出。
    pub fn summary(&self) -> Vec<(&'static str, usize)> {
        let mut counts: Vec<(&'static str, usize)> = Vec::new();
        for d in &self.diagnostics {
            if let Some((_, n)) = counts.iter_mut().find(|(c, _)| *c == d.code) {
                *n += 1;
            } else {
                counts.push((d.code, 1));
            }
        }
        counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        counts
    }
}
