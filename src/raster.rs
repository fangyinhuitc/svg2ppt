//! 光栅化后端（resvg）。
//!
//! 两个用途：
//! 1. `raster` 模式：整页渲染成 PNG，作为一张 `Picture` 铺在幻灯片上。
//! 2. vector 模式的「子树降级」：把无法矢量表达的节点单独渲染成 PNG 贴回原位。

use usvg::tiny_skia_path::Transform;

use crate::geom::{EMU_PER_PX, Placement};

/// 画布输出的像素尺寸（逻辑像素 × 超采样）。
pub fn canvas_pixel_size(width_emu: i64, height_emu: i64, scale: f64) -> (u32, u32) {
    let w = (width_emu as f64 / EMU_PER_PX * scale).round().max(1.0);
    let h = (height_emu as f64 / EMU_PER_PX * scale).round().max(1.0);
    (w as u32, h as u32)
}

/// 按已算好的适配参数把整棵树渲染成 PNG，输出尺寸为 `out_w × out_h`。
///
/// `placement` 的目标是画布 EMU 坐标，这里再乘 `scale / EMU_PER_PX` 换成输出像素坐标。
pub fn render_tree(
    tree: &usvg::Tree,
    placement: &Placement,
    scale: f64,
    out_w: u32,
    out_h: u32,
) -> Option<Vec<u8>> {
    let mut pixmap = tiny_skia::Pixmap::new(out_w.max(1), out_h.max(1))?;
    let k = scale / EMU_PER_PX;
    let transform = Transform::from_row(
        (placement.sx * k) as f32,
        0.0,
        0.0,
        (placement.sy * k) as f32,
        (placement.tx * k) as f32,
        (placement.ty * k) as f32,
    );
    resvg::render(tree, transform, &mut pixmap.as_mut());
    pixmap.encode_png().ok()
}

/// 把单个节点渲染成 PNG，返回 (PNG 字节, 像素宽, 像素高)。
///
/// 渲染范围取节点的 `abs_layer_bounding_box`（已含绝对变换与滤镜膨胀），
/// 调用方据此把结果贴回画布上的对应位置。
pub fn render_node(node: &usvg::Node, scale: f64) -> Option<(Vec<u8>, u32, u32)> {
    let bbox = node.abs_layer_bounding_box()?;
    let w = (bbox.width() as f64 * scale).round().max(1.0) as u32;
    let h = (bbox.height() as f64 * scale).round().max(1.0) as u32;
    let mut pixmap = tiny_skia::Pixmap::new(w, h)?;

    // render_node 内部会 pre_translate(-bbox.x, -bbox.y)，所以这里只给缩放。
    let transform = Transform::from_scale(scale as f32, scale as f32);
    resvg::render_node(node, transform, &mut pixmap.as_mut())?;
    pixmap.encode_png().ok().map(|png| (png, w, h))
}
