//! 核心：把 usvg 树转换成 IR。
//!
//! 这里是整个项目最「有技术含量」的部分——SVG 的表达能力远大于 DrawingML，
//! 所以每个节点的转换前都要先判断「能不能矢量表达」，不能就整棵子树降级为位图。

use std::path::PathBuf;

use usvg::tiny_skia_path::{PathSegment, Point, Transform};

use crate::error::Error;
use crate::geom::{
    EMU_PER_PT, Fit, Placement, Rgb, avg_scale, concat, has_skew_or_rotation, quad_to_cubic,
};
use crate::ir::{
    CapKind, DashKind, Diagnostic, Frame, ImageFormat, ImageItem, Item, JoinKind, Page, Paint,
    PathCmd, RasterItem, Report, ShapeItem, Stop, Stroke, TextHAlign, TextItem, TextRunSpec,
};
use crate::raster;
use crate::svg;

/// 输出模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// 转成真实的 DrawingML 形状（可编辑）。
    #[default]
    Vector,
    /// 整页渲染成位图（保真优先）。
    Raster,
}

/// 画布尺寸。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlideSize {
    Widescreen16x9,
    OnScreen4x3,
    Custom { width_emu: i64, height_emu: i64 },
}

impl SlideSize {
    pub fn emu(&self) -> (i64, i64) {
        match *self {
            SlideSize::Widescreen16x9 => (12_192_000, 6_858_000),
            SlideSize::OnScreen4x3 => (9_144_000, 6_858_000),
            SlideSize::Custom {
                width_emu,
                height_emu,
            } => (width_emu, height_emu),
        }
    }
}

/// 文本处理策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextMode {
    /// **混合**（默认）：能安全表达为 PPT 文本框的用文本框（可编辑），
    /// 其余（多行、旋转、字距调整…）自动回落为字形轮廓，并给出诊断码。
    #[default]
    Auto,
    /// 全部转成字形轮廓（像素级保真，但不可编辑）。
    Path,
}

/// 转换选项。
#[derive(Debug, Clone)]
pub struct ConvertOptions {
    pub mode: Mode,
    pub slide: SlideSize,
    pub fit: Fit,
    /// 背景色，同时作为 alpha 预乘的基底。
    pub background: Option<Rgb>,
    pub text_mode: TextMode,
    /// 位图超采样倍率（相对画布逻辑像素）。
    pub scale: f64,
    pub font_dirs: Vec<PathBuf>,
}

impl Default for ConvertOptions {
    fn default() -> Self {
        Self {
            mode: Mode::Vector,
            slide: SlideSize::Widescreen16x9,
            fit: Fit::Contain,
            background: None,
            text_mode: TextMode::Auto,
            scale: 2.0,
            font_dirs: Vec::new(),
        }
    }
}

impl ConvertOptions {
    pub fn canvas(&self) -> (i64, i64) {
        self.slide.emu()
    }
}

/// 把一份 SVG 转成 IR 页面。
pub fn svg_to_page(svg: &[u8], opts: &ConvertOptions) -> Result<(Page, Report), Error> {
    let tree = svg::parse(svg, &opts.font_dirs)?;
    Ok(tree_to_page(&tree, opts))
}

/// 把已解析的树转成 IR 页面。
pub fn tree_to_page(tree: &usvg::Tree, opts: &ConvertOptions) -> (Page, Report) {
    let mut report = Report::default();
    let (width_emu, height_emu) = opts.canvas();
    let mut page = Page {
        width_emu,
        height_emu,
        background: opts.background,
        items: Vec::new(),
    };

    let placement = Placement::compute(
        tree.size().width() as f64,
        tree.size().height() as f64,
        width_emu,
        height_emu,
        opts.fit,
    );

    if opts.mode == Mode::Raster {
        let (out_w, out_h) = raster::canvas_pixel_size(width_emu, height_emu, opts.scale);
        let png = raster::render_tree(tree, &placement, opts.scale, out_w, out_h)
            .ok_or_else(|| Error::Raster("整页渲染失败".into()));
        match png {
            Ok(png) => page.items.push(Item::Raster(RasterItem {
                name: "svg".into(),
                frame: Frame {
                    x: 0,
                    y: 0,
                    w: width_emu,
                    h: height_emu,
                },
                png,
            })),
            Err(e) => report.push("raster-failed", e.to_string()),
        }
        return (page, report);
    }

    let mut walker = Walker {
        place: placement.to_transform(),
        place_scale: placement.scale(),
        supersample: opts.scale.max(1.0),
        bg: opts.background.unwrap_or(Rgb::WHITE),
        text_mode: opts.text_mode,
        page: &mut page,
        report: &mut report,
        next_id: 0,
    };
    // 根 group 只承载元数据（它的变换已经累积进子节点的 abs_transform），直接遍历子节点即可。
    for child in tree.root().children() {
        walker.walk(child, Transform::identity(), 1.0);
    }

    (page, report)
}

struct Walker<'a> {
    /// SVG px → 画布 EMU。
    place: Transform,
    /// place 的平均缩放（SVG px → 画布 px），用于位图分辨率。
    place_scale: f64,
    supersample: f64,
    bg: Rgb,
    text_mode: TextMode,
    page: &'a mut Page,
    report: &'a mut Report,
    next_id: usize,
}

impl Walker<'_> {
    fn next_name(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{prefix}{}", self.next_id)
    }

    /// `extra` 是在节点自身绝对变换**之后**再叠加的变换（文本展平子树会用到）。
    fn walk(&mut self, node: &usvg::Node, extra: Transform, parent_opacity: f64) {
        match node {
            usvg::Node::Group(g) => {
                if self.group_needs_raster(g) {
                    let reason = if !g.filters().is_empty() {
                        "filter-rasterized"
                    } else if g.clip_path().is_some() {
                        "clip-path-rasterized"
                    } else {
                        "mask-rasterized"
                    };
                    self.report.push(
                        reason,
                        format!(
                            "分组 {:?} 含 DrawingML 无法表达的特性，已降级为位图",
                            g.id()
                        ),
                    );
                    self.rasterize(node);
                    return;
                }
                let opacity = parent_opacity * g.opacity().get() as f64;
                if opacity <= 0.001 {
                    return;
                }
                if opacity < 0.999 {
                    self.report.push(
                        "group-opacity-flattened",
                        "分组透明度被近似到子元素上（组内元素重叠时会有细微差异）",
                    );
                }
                // 分组本身不产生几何：它的变换已经累积到子节点的 abs_transform 里。
                for child in g.children() {
                    self.walk(child, extra, opacity);
                }
            }

            usvg::Node::Path(p) => {
                if let Some(fill) = p.fill() {
                    if matches!(fill.paint(), usvg::Paint::Pattern(_)) {
                        self.report.push(
                            "pattern-rasterized",
                            "图案填充（pattern）无法用 DrawingML 表达，该形状已降级为位图",
                        );
                        self.rasterize(node);
                        return;
                    }
                    if matches!(fill.paint(), usvg::Paint::RadialGradient(_)) {
                        self.report.push(
                            "radial-gradient-rasterized",
                            "径向渐变无法用 DrawingML 的线性渐变表达，该形状已降级为位图",
                        );
                        self.rasterize(node);
                        return;
                    }
                }
                let total = concat(concat(p.abs_transform(), extra), self.place);
                let scale = avg_scale(total);
                let opacity = parent_opacity;

                let fill = match p.fill() {
                    Some(f) => match self.convert_paint(
                        f.paint(),
                        f.opacity().get() as f64 * opacity,
                        total,
                    ) {
                        Ok(paint) => Some(paint),
                        Err(code) => {
                            self.report.push(code, "填充无法矢量表达，形状已降级为位图");
                            self.rasterize(node);
                            return;
                        }
                    },
                    None => None,
                };

                let stroke = match p.stroke() {
                    Some(s) => {
                        // 注意：`scale` 已经是「1 SVG px = 多少 EMU」（total 里带了 EMU 换算），
                        // 这里不能再走 px_to_emu，否则会二次换算。
                        let width_emu = (s.width().get() as f64 * scale).round().max(1.0) as i64;
                        match self.convert_paint(
                            s.paint(),
                            s.opacity().get() as f64 * opacity,
                            total,
                        ) {
                            Ok(paint) => {
                                let dash = s.dasharray().and_then(|d| {
                                    let kind = map_dash(d, s.width().get() as f64);
                                    if kind.is_some() {
                                        self.report.push(
                                            "dash-approximated",
                                            "自定义虚线样式已就近映射为 PowerPoint 的预设虚线",
                                        );
                                    }
                                    kind
                                });
                                Some(Stroke {
                                    paint,
                                    width_emu,
                                    cap: match s.linecap() {
                                        usvg::LineCap::Butt => CapKind::Butt,
                                        usvg::LineCap::Round => CapKind::Round,
                                        usvg::LineCap::Square => CapKind::Square,
                                    },
                                    join: match s.linejoin() {
                                        // MiterClip 与 Miter 在 DrawingML 里没有区别，一并按 Miter 处理。
                                        usvg::LineJoin::Miter | usvg::LineJoin::MiterClip => {
                                            JoinKind::Miter
                                        }
                                        usvg::LineJoin::Round => JoinKind::Round,
                                        usvg::LineJoin::Bevel => JoinKind::Bevel,
                                    },
                                    dash,
                                })
                            }
                            Err(code) => {
                                self.report.push(code, "描边无法矢量表达，形状已降级为位图");
                                self.rasterize(node);
                                return;
                            }
                        }
                    }
                    None => None,
                };

                if let Some(item) = self.build_shape(p, total, fill, stroke) {
                    self.page.items.push(Item::Shape(item));
                }
            }

            usvg::Node::Image(img) => {
                if has_skew_or_rotation(img.abs_transform()) {
                    self.report.push(
                        "rotated-image-rasterized",
                        "PowerPoint 的图片形状不支持旋转/斜切，该图片已降级为位图",
                    );
                    self.rasterize(node);
                    return;
                }
                let total = concat(concat(img.abs_transform(), extra), self.place);
                // 注意：usvg 的 `Image::abs_bounding_box` 只叠加了**父级**变换，
                // 不含图片自身的 `image_ts`（见 usvg parser/image.rs），所以这里必须用局部
                // bbox 再乘 `total`，否则会被重复平移一次。
                let bbox = img.bounding_box();
                let frame =
                    self.frame_of(bbox.left(), bbox.top(), bbox.width(), bbox.height(), total);

                let (data, format) = match img.kind() {
                    usvg::ImageKind::PNG(data) => (data.as_slice(), ImageFormat::Png),
                    usvg::ImageKind::JPEG(data) => (data.as_slice(), ImageFormat::Jpeg),
                    usvg::ImageKind::GIF(data) => (data.as_slice(), ImageFormat::Gif),
                    _ => {
                        self.report.push(
                            "unsupported-image-rasterized",
                            "PPTX 只接受 PNG/JPEG/GIF，其它格式（WebP/嵌套 SVG）已降级为位图",
                        );
                        self.rasterize(node);
                        return;
                    }
                };

                let name = self.next_name("图片 ");
                self.page.items.push(Item::Image(ImageItem {
                    name,
                    frame,
                    data: data.to_vec(),
                    format,
                }));
            }

            usvg::Node::Text(t) => {
                let total = concat(concat(t.abs_transform(), extra), self.place);
                if self.text_mode == TextMode::Auto {
                    match self.try_text_item(t, total, parent_opacity) {
                        Ok(item) => {
                            self.page.items.push(Item::Text(item));
                            return;
                        }
                        Err(code) => {
                            self.report.push(
                                code,
                                format!(
                                    "文本 {:?} 无法用 PPT 文本框精确表达，已回落为字形轮廓（不可编辑）",
                                    t.id()
                                ),
                            );
                        }
                    }
                } else {
                    self.report.push(
                        "text-as-path",
                        format!(
                            "文本 {:?} 按 --text-mode path 转曲（保真但不可编辑）",
                            t.id()
                        ),
                    );
                }
                // flattened() 给出的是「排版好的字形轮廓」子树。
                let flat = t.flattened();
                // 展平子树的根变换若是单位矩阵，说明它没带上 text 节点自身的变换，需要手动叠加。
                let extra2 = if flat.abs_transform().is_identity() {
                    concat(t.abs_transform(), extra)
                } else {
                    extra
                };
                for child in flat.children() {
                    self.walk(child, extra2, parent_opacity);
                }
            }
        }
    }

    /// 尝试把文本表达成一个可编辑的 PPT 文本框。
    ///
    /// PPT 的排版引擎会按自己的字体度量重排，所以只有在「排版结果不会漂移」时才能走这条路：
    /// 单行、水平、无旋转/斜切、无字距与基线调整、非描边、纯色填充。
    /// 任何一条不满足都返回诊断码，由调用方回落为字形轮廓。
    fn try_text_item(
        &mut self,
        t: &usvg::Text,
        total: Transform,
        opacity: f64,
    ) -> Result<TextItem, &'static str> {
        let chunks = t.chunks();
        // 多行需要 PPT 自己算行距，几乎必然与 SVG 不一致，直接转曲。
        if chunks.len() != 1 {
            return Err("text-multiline-fallback");
        }
        let chunk = &chunks[0];
        if !matches!(chunk.text_flow(), usvg::TextFlow::Linear) {
            return Err("text-on-path-fallback");
        }
        if !matches!(t.writing_mode(), usvg::WritingMode::LeftToRight) {
            return Err("text-vertical-fallback");
        }
        if has_skew_or_rotation(total) {
            return Err("text-rotated-fallback");
        }
        if t.rotate().iter().any(|r| *r != 0.0) {
            return Err("text-rotated-fallback");
        }
        if t.dx().iter().any(|v| *v != 0.0) || t.dy().iter().any(|v| *v != 0.0) {
            return Err("text-per-glyph-offset-fallback");
        }
        let scale = avg_scale(total);
        let mut runs: Vec<TextRunSpec> = Vec::new();
        for span in chunk.spans() {
            if !span.is_visible() {
                return Err("text-hidden-fallback");
            }
            if span.stroke().is_some() {
                return Err("text-stroked-fallback");
            }
            if span.letter_spacing() != 0.0 || span.word_spacing() != 0.0 {
                return Err("text-spacing-fallback");
            }
            if span.text_length().is_some() {
                return Err("text-length-adjust-fallback");
            }
            if !span.baseline_shift().is_empty() {
                return Err("text-baseline-shift-fallback");
            }
            if span.small_caps() {
                return Err("text-small-caps-fallback");
            }

            let color = match span.fill() {
                Some(f) => match f.paint() {
                    usvg::Paint::Color(c) => Rgb {
                        r: c.red,
                        g: c.green,
                        b: c.blue,
                    }
                    .blend_over(f.opacity().get() as f64 * opacity, self.bg),
                    _ => return Err("text-paint-fallback"),
                },
                None => Rgb::BLACK.blend_over(opacity, self.bg),
            };

            let text = chunk
                .text()
                .get(span.start()..span.end())
                .unwrap_or_default()
                .to_string();
            if text.is_empty() {
                continue;
            }

            // scale 已是「1 SVG px = 多少 EMU」，所以 px → EMU 只需乘它。
            let size_emu = span.font_size().get() as f64 * scale;
            let size_100ths_pt = (size_emu / EMU_PER_PT * 100.0)
                .round()
                .clamp(1.0, 400_000.0);
            let deco = span.decoration();
            runs.push(TextRunSpec {
                text,
                font_family: first_font_family(span.font()),
                size_100ths_pt: size_100ths_pt as i32,
                bold: span.font().weight() >= 600,
                italic: !matches!(span.font().style(), usvg::FontStyle::Normal),
                underline: deco.underline().is_some(),
                strike: deco.line_through().is_some(),
                color,
            });
        }
        if runs.is_empty() {
            return Err("text-empty-fallback");
        }

        // SVG 的 text bbox 是「排版盒」（上边界 = 基线 - 字体 ascent，高 = ascent + descent），
        // 恰好对应 PPT 文本框的行盒语义，比字形紧包围盒更适合拿来当文本框的框。
        let bbox = t.bounding_box();
        Ok(TextItem {
            name: self.next_name("文本 "),
            frame: self.frame_of(bbox.left(), bbox.top(), bbox.width(), bbox.height(), total),
            align: match chunk.anchor() {
                usvg::TextAnchor::Start => TextHAlign::Left,
                usvg::TextAnchor::Middle => TextHAlign::Center,
                usvg::TextAnchor::End => TextHAlign::Right,
            },
            runs,
        })
    }

    fn group_needs_raster(&self, g: &usvg::Group) -> bool {
        !g.filters().is_empty() || g.clip_path().is_some() || g.mask().is_some()
    }

    /// 把节点渲染成 PNG 并贴回它在画布上的位置。
    fn rasterize(&mut self, node: &usvg::Node) {
        let scale = self.place_scale * self.supersample;
        let Some((png, _w, _h)) = raster::render_node(node, scale) else {
            self.report
                .push("raster-failed", "局部光栅化失败，该节点被跳过");
            return;
        };
        // render_node 的取景框是 abs_layer_bounding_box，按同一变换贴回画布。
        let Some(bbox) = node.abs_layer_bounding_box() else {
            self.report.push("raster-failed", "节点包围盒为空，已跳过");
            return;
        };
        let frame = self.frame_of(bbox.x(), bbox.y(), bbox.width(), bbox.height(), self.place);
        let name = self.next_name("位图 ");
        self.page
            .items
            .push(Item::Raster(RasterItem { name, frame, png }));
    }

    /// 把 SVG 坐标下的矩形经 `total` 变换后转成画布 EMU 框。
    fn frame_of(&self, x: f32, y: f32, w: f32, h: f32, total: Transform) -> Frame {
        let corners = [
            Point::from_xy(x, y),
            Point::from_xy(x + w, y),
            Point::from_xy(x + w, y + h),
            Point::from_xy(x, y + h),
        ];
        let mut xs = [f64::MAX; 4];
        let mut ys = [f64::MAX; 4];
        for (i, c) in corners.iter().enumerate() {
            let mut p = *c;
            total.map_point(&mut p);
            xs[i] = p.x as f64;
            ys[i] = p.y as f64;
        }
        let min_x = xs.iter().cloned().fold(f64::INFINITY, f64::min);
        let max_x = xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let min_y = ys.iter().cloned().fold(f64::INFINITY, f64::min);
        let max_y = ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        Frame {
            x: min_x.round() as i64,
            y: min_y.round() as i64,
            w: (max_x - min_x).round().max(1.0) as i64,
            h: (max_y - min_y).round().max(1.0) as i64,
        }
    }

    /// 路径 → 形状。逐点变换后再算 bbox，保证任意仿射变换下位置精确。
    fn build_shape(
        &mut self,
        path: &usvg::Path,
        total: Transform,
        fill: Option<Paint>,
        stroke: Option<Stroke>,
    ) -> Option<ShapeItem> {
        let mut cmds: Vec<PathCmd> = Vec::new();
        let mut min_x = f64::INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut max_y = f64::NEG_INFINITY;

        let mut track = |p: Point| {
            min_x = min_x.min(p.x as f64);
            min_y = min_y.min(p.y as f64);
            max_x = max_x.max(p.x as f64);
            max_y = max_y.max(p.y as f64);
        };
        let map = |p: Point| {
            let mut q = p;
            total.map_point(&mut q);
            q
        };

        let mut cur = Point::from_xy(0.0, 0.0);
        for seg in path.data().segments() {
            match seg {
                PathSegment::MoveTo(p) => {
                    cur = p;
                    let q = map(p);
                    track(q);
                    cmds.push(PathCmd::MoveTo(q.x.round() as i64, q.y.round() as i64));
                }
                PathSegment::LineTo(p) => {
                    cur = p;
                    let q = map(p);
                    track(q);
                    cmds.push(PathCmd::LineTo(q.x.round() as i64, q.y.round() as i64));
                }
                PathSegment::QuadTo(c, p) => {
                    let (c1, c2) = quad_to_cubic(cur, c, p);
                    cur = p;
                    let a = map(c1);
                    let b = map(c2);
                    let q = map(p);
                    track(a);
                    track(b);
                    track(q);
                    cmds.push(PathCmd::CubicTo(
                        a.x.round() as i64,
                        a.y.round() as i64,
                        b.x.round() as i64,
                        b.y.round() as i64,
                        q.x.round() as i64,
                        q.y.round() as i64,
                    ));
                }
                PathSegment::CubicTo(c1, c2, p) => {
                    cur = p;
                    let a = map(c1);
                    let b = map(c2);
                    let q = map(p);
                    track(a);
                    track(b);
                    track(q);
                    cmds.push(PathCmd::CubicTo(
                        a.x.round() as i64,
                        a.y.round() as i64,
                        b.x.round() as i64,
                        b.y.round() as i64,
                        q.x.round() as i64,
                        q.y.round() as i64,
                    ));
                }
                PathSegment::Close => cmds.push(PathCmd::Close),
            }
        }

        if cmds.is_empty() || !min_x.is_finite() {
            return None;
        }

        // 把描边宽度的一半算进边框，避免描边溢出形状选择框。
        let pad = stroke
            .as_ref()
            .map(|s| s.width_emu as f64 / 2.0)
            .unwrap_or(0.0);
        let x = (min_x - pad).floor();
        let y = (min_y - pad).floor();
        let w = ((max_x + pad).ceil() - x).max(1.0);
        let h = ((max_y + pad).ceil() - y).max(1.0);
        let origin_x = x as i64;
        let origin_y = y as i64;

        // 指令坐标改为相对形状左上角。
        for cmd in &mut cmds {
            match cmd {
                PathCmd::MoveTo(px, py) | PathCmd::LineTo(px, py) => {
                    *px -= origin_x;
                    *py -= origin_y;
                }
                PathCmd::CubicTo(a, b, c, d, e, f) => {
                    *a -= origin_x;
                    *b -= origin_y;
                    *c -= origin_x;
                    *d -= origin_y;
                    *e -= origin_x;
                    *f -= origin_y;
                }
                PathCmd::Close => {}
            }
        }

        Some(ShapeItem {
            name: self.next_name("形状 "),
            frame: Frame {
                x: origin_x,
                y: origin_y,
                w: w as i64,
                h: h as i64,
            },
            commands: cmds,
            fill,
            stroke,
        })
    }

    /// SVG 颜料 → IR 颜料。
    ///
    /// 返回 `Err(诊断码)` 表示这个颜料无法矢量表达，调用方应改走位图降级。
    fn convert_paint(
        &mut self,
        paint: &usvg::Paint,
        opacity: f64,
        total: Transform,
    ) -> Result<Paint, &'static str> {
        // DrawingML 的颜色没有 alpha 通道，半透明只能靠与背景色预乘来近似——必须显式告知。
        if opacity < 0.999 {
            self.report.push(
                "alpha-approximated",
                "半透明颜色与背景色预乘为不透明色（DrawingML 的 srgbClr 不支持 alpha 通道）",
            );
        }
        match paint {
            usvg::Paint::Color(c) => {
                let rgb = Rgb {
                    r: c.red,
                    g: c.green,
                    b: c.blue,
                };
                Ok(Paint::Solid(rgb.blend_over(opacity, self.bg)))
            }
            usvg::Paint::LinearGradient(lg) => {
                // 渐变向量同样要过一遍变换，旋转/缩放后才不会错位。
                let mut p1 = Point::from_xy(lg.x1(), lg.y1());
                let mut p2 = Point::from_xy(lg.x2(), lg.y2());
                total.map_point(&mut p1);
                total.map_point(&mut p2);
                let angle_deg = (p2.y - p1.y).atan2(p2.x - p1.x).to_degrees() as f64;

                let stops = lg
                    .stops()
                    .iter()
                    .map(|s| Stop {
                        pos: s.offset().get() as f64,
                        color: Rgb {
                            r: s.color().red,
                            g: s.color().green,
                            b: s.color().blue,
                        }
                        .blend_over(s.opacity().get() as f64 * opacity, self.bg),
                    })
                    .collect();

                if lg.spread_method() != usvg::SpreadMethod::Pad {
                    self.report.push(
                        "gradient-spread-approximated",
                        "渐变的 reflect/repeat 扩散方式无法表达，已按 pad 处理",
                    );
                }
                Ok(Paint::LinearGradient { angle_deg, stops })
            }
            usvg::Paint::RadialGradient(_) => Err("radial-gradient-rasterized"),
            usvg::Paint::Pattern(_) => Err("pattern-rasterized"),
        }
    }
}

/// 把 SVG 的 `stroke-dasharray` 就近映射到 PowerPoint 的 11 种预设虚线之一。
fn map_dash(pattern: &[f32], width: f64) -> Option<DashKind> {
    if pattern.is_empty() || width <= 0.0 {
        return None;
    }
    let on = pattern[0] as f64 / width;
    let has_dot = pattern.len() >= 4 && (pattern[2] as f64 / width) < 1.5;
    Some(match (on, has_dot) {
        (o, false) if o < 1.5 => DashKind::Dot,
        (o, false) if o < 4.0 => DashKind::Dash,
        (_, false) => DashKind::LargeDash,
        (o, true) if o < 4.0 => DashKind::DashDot,
        (o, true) if o < 8.0 => DashKind::LargeDashDot,
        (_, true) => DashKind::LargeDashDotDot,
    })
}

/// 取字体栈里第一个可用族名；CSS 通用族映射成 PowerPoint 上确实存在的字体。
fn first_font_family(font: &usvg::Font) -> Option<String> {
    // 字体栈里每个候选项都能得到一个名字，所以取第一个即可。
    font.families()
        .iter()
        .map(|f| match f {
            usvg::FontFamily::Named(name) => name.clone(),
            usvg::FontFamily::SansSerif => "Arial".to_string(),
            usvg::FontFamily::Serif => "Times New Roman".to_string(),
            usvg::FontFamily::Monospace => "Consolas".to_string(),
            usvg::FontFamily::Cursive => "Comic Sans MS".to_string(),
            usvg::FontFamily::Fantasy => "Impact".to_string(),
        })
        .next()
}

/// 供测试与上层复用：一次转换后拿到的诊断。
pub fn diagnostics_of(report: &Report) -> Vec<Diagnostic> {
    report.diagnostics.clone()
}
