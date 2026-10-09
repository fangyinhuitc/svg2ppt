//! 单位换算、画布适配与仿射矩阵工具。
//!
//! DrawingML 的唯一长度单位是 EMU（English Metric Units）。本模块负责把 SVG 的用户
//! 单位（px）与各种角度/百分比约定转换成 OOXML 的 EMU / 1-60000 度 / 千分之一百分比。

use usvg::tiny_skia_path::{Point, Transform};

/// 1 英寸 = 914 400 EMU。
pub const EMU_PER_INCH: i64 = 914_400;

/// CSS 像素：1 px = 1/96 英寸 = 9 525 EMU。
pub const EMU_PER_PX: f64 = EMU_PER_INCH as f64 / 96.0;

/// 排版磅值：1 pt = 1/72 英寸 = 12 700 EMU。DrawingML 的字号 `sz` 以 1/100 pt 为单位。
pub const EMU_PER_PT: f64 = EMU_PER_INCH as f64 / 72.0;

/// DrawingML 把角度存成 1/60000 度。
pub const UNITS_PER_DEGREE: f64 = 60_000.0;

/// 把 SVG 的 px 长度转成 EMU。
pub fn px_to_emu(v: f64) -> i64 {
    (v * EMU_PER_PX).round() as i64
}

/// 8 位 sRGB 颜色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const WHITE: Rgb = Rgb {
        r: 255,
        g: 255,
        b: 255,
    };
    pub const BLACK: Rgb = Rgb { r: 0, g: 0, b: 0 };

    /// 转成 DrawingML `<a:srgbClr val=".."/>` 需要的 6 位大写十六进制。
    pub fn hex(&self) -> String {
        format!("{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }

    /// 解析 `#rgb` / `#rrggbb` / 常见颜色名。
    pub fn parse(s: &str) -> Option<Rgb> {
        let s = s.trim();
        if let Some(hex) = s.strip_prefix('#') {
            return match hex.len() {
                3 => {
                    let mut it = hex.chars();
                    let expand = |c: char| u8::from_str_radix(&format!("{c}{c}"), 16).ok();
                    Some(Rgb {
                        r: expand(it.next()?)?,
                        g: expand(it.next()?)?,
                        b: expand(it.next()?)?,
                    })
                }
                6 => Some(Rgb {
                    r: u8::from_str_radix(&hex[0..2], 16).ok()?,
                    g: u8::from_str_radix(&hex[2..4], 16).ok()?,
                    b: u8::from_str_radix(&hex[4..6], 16).ok()?,
                }),
                _ => None,
            };
        }
        Some(match s.to_ascii_lowercase().as_str() {
            "white" => Rgb::WHITE,
            "black" => Rgb::BLACK,
            "transparent" => Rgb::WHITE,
            "red" => Rgb { r: 255, g: 0, b: 0 },
            "green" => Rgb { r: 0, g: 128, b: 0 },
            "blue" => Rgb { r: 0, g: 0, b: 255 },
            "gray" | "grey" => Rgb {
                r: 128,
                g: 128,
                b: 128,
            },
            _ => return None,
        })
    }

    /// 与背景色按 alpha 预乘。
    ///
    /// `Color` 在 DrawingML 里不带 alpha 通道，SVG 的 `opacity` / `fill-opacity`
    /// 只能靠把颜色混入背景色来近似（见设计文档 6.3）。
    pub fn blend_over(&self, alpha: f64, bg: Rgb) -> Rgb {
        let a = alpha.clamp(0.0, 1.0);
        let mix = |fg: u8, b: u8| (fg as f64 * a + b as f64 * (1.0 - a)).round() as u8;
        Rgb {
            r: mix(self.r, bg.r),
            g: mix(self.g, bg.g),
            b: mix(self.b, bg.b),
        }
    }
}

/// SVG 内容放进画布的方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Fit {
    /// 等比缩放并居中，留白。
    #[default]
    Contain,
    /// 等比缩放并居中，超出画布的部分被裁掉。
    Cover,
    /// 非等比拉伸到铺满画布。
    Stretch,
    /// 1:1（px → EMU），不缩放。
    None,
}

/// 把 SVG 用户坐标映射到画布坐标的仿射变换（单位仍是 px，最后统一乘 EMU_PER_PX）。
#[derive(Debug, Clone, Copy)]
pub struct Placement {
    pub sx: f64,
    pub sy: f64,
    pub tx: f64,
    pub ty: f64,
}

impl Placement {
    /// 计算适配参数。
    ///
    /// `canvas_*_emu` 是画布尺寸；内部先换算回 px 空间求缩放比，再回到 EMU 空间取整偏移。
    pub fn compute(svg_w: f64, svg_h: f64, canvas_w_emu: i64, canvas_h_emu: i64, fit: Fit) -> Self {
        if svg_w <= 0.0 || svg_h <= 0.0 {
            return Self {
                sx: 1.0,
                sy: 1.0,
                tx: 0.0,
                ty: 0.0,
            };
        }
        let canvas_w = canvas_w_emu as f64 / EMU_PER_PX;
        let canvas_h = canvas_h_emu as f64 / EMU_PER_PX;
        let (sx, sy) = match fit {
            Fit::Contain => {
                let s = (canvas_w / svg_w).min(canvas_h / svg_h);
                (s, s)
            }
            Fit::Cover => {
                let s = (canvas_w / svg_w).max(canvas_h / svg_h);
                (s, s)
            }
            Fit::Stretch => (canvas_w / svg_w, canvas_h / svg_h),
            Fit::None => (1.0, 1.0),
        };
        // 居中（Cover 时偏移为负，即裁掉溢出部分）。
        let tx = (canvas_w - svg_w * sx) / 2.0;
        let ty = (canvas_h - svg_h * sy) / 2.0;
        Self { sx, sy, tx, ty }
    }

    /// 转成「SVG px → 画布 EMU」的矩阵。
    pub fn to_transform(&self) -> Transform {
        Transform::from_row(
            (self.sx * EMU_PER_PX) as f32,
            0.0,
            0.0,
            (self.sy * EMU_PER_PX) as f32,
            (self.tx * EMU_PER_PX) as f32,
            (self.ty * EMU_PER_PX) as f32,
        )
    }

    /// 只取缩放部分（用于把局部区域渲染成位图时的像素尺寸计算）。
    pub fn scale(&self) -> f64 {
        (self.sx.abs() + self.sy.abs()) / 2.0
    }
}

/// 把 `Transform` 拆成 `[sx, ky, kx, sy, tx, ty]`。
///
/// 不直接读字段是为了避免依赖 tiny-skia 的字段可见性：用三个探针点即可反解出矩阵。
pub fn decompose(t: Transform) -> [f64; 6] {
    let mut o = Point::from_xy(0.0, 0.0);
    let mut x = Point::from_xy(1.0, 0.0);
    let mut y = Point::from_xy(0.0, 1.0);
    t.map_point(&mut o);
    t.map_point(&mut x);
    t.map_point(&mut y);
    [
        (x.x - o.x) as f64,
        (x.y - o.y) as f64,
        (y.x - o.x) as f64,
        (y.y - o.y) as f64,
        o.x as f64,
        o.y as f64,
    ]
}

/// 组合两个变换：结果等价于「先应用 `a`，再应用 `b`」。
pub fn concat(a: Transform, b: Transform) -> Transform {
    let a = decompose(a);
    let b = decompose(b);
    let (a_sx, a_ky, a_kx, a_sy, a_tx, a_ty) = (a[0], a[1], a[2], a[3], a[4], a[5]);
    let (b_sx, b_ky, b_kx, b_sy, b_tx, b_ty) = (b[0], b[1], b[2], b[3], b[4], b[5]);
    Transform::from_row(
        (b_sx * a_sx + b_kx * a_ky) as f32,
        (b_ky * a_sx + b_sy * a_ky) as f32,
        (b_sx * a_kx + b_kx * a_sy) as f32,
        (b_ky * a_kx + b_sy * a_sy) as f32,
        (b_sx * a_tx + b_kx * a_ty + b_tx) as f32,
        (b_ky * a_tx + b_sy * a_ty + b_ty) as f32,
    )
}

/// 是否含旋转或斜切（即非「平移 + 轴对齐缩放」）。
pub fn has_skew_or_rotation(t: Transform) -> bool {
    let d = decompose(t);
    d[1].abs() > 1e-4 || d[2].abs() > 1e-4
}

/// 平均缩放因子（`sqrt(|det|)`），用于描边宽度、字号这类「各向等宽」的量。
pub fn avg_scale(t: Transform) -> f64 {
    let d = decompose(t);
    (d[0] * d[3] - d[1] * d[2]).abs().sqrt().max(1e-6)
}

/// 二次贝塞尔升阶为三次贝塞尔（数学上等价）。
///
/// DrawingML 的 `a:custGeom` 只有 `cubicBezTo`，没有 `quadBezTo`。
pub fn quad_to_cubic(p0: Point, q: Point, p: Point) -> (Point, Point) {
    let c1 = Point::from_xy(
        p0.x + 2.0 / 3.0 * (q.x - p0.x),
        p0.y + 2.0 / 3.0 * (q.y - p0.y),
    );
    let c2 = Point::from_xy(p.x + 2.0 / 3.0 * (q.x - p.x), p.y + 2.0 / 3.0 * (q.y - p.y));
    (c1, c2)
}

/// 把「SVG 意义上的角度（度，顺时针为正，y 轴向下）」转成 DrawingML 的 1/60000 度。
pub fn degrees_to_units(deg: f64) -> i32 {
    (deg * UNITS_PER_DEGREE).round() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emu_conversion() {
        assert_eq!(px_to_emu(96.0), EMU_PER_INCH);
        assert_eq!(px_to_emu(1.0), 9525);
    }

    #[test]
    fn hex_and_parse() {
        assert_eq!(
            Rgb {
                r: 255,
                g: 0,
                b: 128
            }
            .hex(),
            "FF0080"
        );
        assert_eq!(
            Rgb::parse("#f08"),
            Some(Rgb {
                r: 255,
                g: 0,
                b: 136
            })
        );
        assert_eq!(Rgb::parse("white"), Some(Rgb::WHITE));
        assert_eq!(Rgb::parse("nope"), None);
    }

    #[test]
    fn alpha_blends_against_background() {
        let c = Rgb { r: 255, g: 0, b: 0 }.blend_over(0.5, Rgb::WHITE);
        assert_eq!(
            c,
            Rgb {
                r: 255,
                g: 128,
                b: 128
            }
        );
        // alpha = 1 时保持原色，alpha = 0 时完全变成背景色。
        assert_eq!(Rgb::BLACK.blend_over(1.0, Rgb::WHITE), Rgb::BLACK);
        assert_eq!(Rgb::BLACK.blend_over(0.0, Rgb::WHITE), Rgb::WHITE);
    }

    #[test]
    fn contain_keeps_aspect_and_centers() {
        // 100x100 的 SVG 放进 16:9 画布：等比缩放后高度撑满，左右居中。
        let p = Placement::compute(100.0, 100.0, 12_192_000, 6_858_000, Fit::Contain);
        assert!((p.sx - p.sy).abs() < 1e-9);
        let drawn_h = 100.0 * p.sy;
        let drawn_w = 100.0 * p.sx;
        assert!((drawn_h - 6_858_000.0 / EMU_PER_PX).abs() < 1e-6);
        assert!((p.tx - (12_192_000.0 / EMU_PER_PX - drawn_w) / 2.0).abs() < 1e-6);
    }

    #[test]
    fn concat_composes_in_order() {
        let translate = Transform::from_translate(10.0, 0.0);
        let scale = Transform::from_scale(2.0, 2.0);
        // 先平移再缩放 → 原点落在 (20, 0)。
        let mut pt = Point::from_xy(0.0, 0.0);
        concat(translate, scale).map_point(&mut pt);
        assert!((pt.x - 20.0).abs() < 1e-4);
        // 先缩放再平移 → 原点落在 (10, 0)。
        let mut pt = Point::from_xy(0.0, 0.0);
        concat(scale, translate).map_point(&mut pt);
        assert!((pt.x - 10.0).abs() < 1e-4);
    }

    #[test]
    fn quad_to_cubic_is_equivalent() {
        let p0 = Point::from_xy(0.0, 0.0);
        let q = Point::from_xy(10.0, 0.0);
        let p = Point::from_xy(10.0, 10.0);
        let (c1, c2) = quad_to_cubic(p0, q, p);
        // 三次贝塞尔在 t=0.5 处应落在二次贝塞尔的同一点上。
        let quad_at_half = Point::from_xy(
            0.25 * p0.x + 0.5 * q.x + 0.25 * p.x,
            0.25 * p0.y + 0.5 * q.y + 0.25 * p.y,
        );
        let cubic_at_half = Point::from_xy(
            0.125 * p0.x + 0.375 * c1.x + 0.375 * c2.x + 0.125 * p.x,
            0.125 * p0.y + 0.375 * c1.y + 0.375 * c2.y + 0.125 * p.y,
        );
        assert!((quad_at_half.x - cubic_at_half.x).abs() < 1e-4);
        assert!((quad_at_half.y - cubic_at_half.y).abs() < 1e-4);
    }

    #[test]
    fn skew_detection() {
        assert!(!has_skew_or_rotation(Transform::from_scale(2.0, 3.0)));
        assert!(has_skew_or_rotation(Transform::from_rotate(30.0)));
    }
}
