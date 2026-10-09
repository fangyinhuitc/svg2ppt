//! IR → PPTX。
//!
//! 这是全项目唯一依赖 `office-toolkit` 的模块。换后端时只需要重写这里。

use std::io::Cursor;

use office_toolkit::drawing::{
    Color, CustomGeometry, Fill, Geometry, GradientFill, GradientStop, Line, LineCap, LineJoin,
    PathCommand, PresetLineDash, ShapeProperties, TextAlign, TextAnchor, TextAutofit, TextBody,
    TextBodyProperties, TextParagraph, TextParagraphProperties, TextRun, TextRunProperties,
    TextStrike, TextUnderline, TextWrap, Transform2D,
};
use office_toolkit::powerpoint::{AutoShape, Picture, PictureFormat, Presentation, Shape, Slide};

use crate::error::Error;
use crate::geom::degrees_to_units;
use crate::ir::{
    CapKind, DashKind, ImageFormat, Item, JoinKind, Page, Paint, PathCmd, Stroke, TextHAlign,
    TextItem,
};

/// 把若干页打包成一个 pptx。
pub fn build_pptx(pages: &[Page]) -> Result<Vec<u8>, Error> {
    let mut presentation = Presentation::new();
    if let Some(first) = pages.first() {
        presentation.slide_width_emu = first.width_emu;
        presentation.slide_height_emu = first.height_emu;
    }
    for page in pages {
        presentation.slides.push(build_slide(page));
    }
    let cursor = Cursor::new(Vec::new());
    let cursor = presentation
        .write_to(cursor)
        .map_err(|e| Error::Pptx(e.to_string()))?;
    Ok(cursor.into_inner())
}

fn build_slide(page: &Page) -> Slide {
    let mut slide = Slide::new();
    // id = 1 被 writer 保留给幻灯片自身的 group shape。
    let mut id: u32 = 2;

    if let Some(bg) = page.background {
        slide
            .shapes
            .push(Shape::AutoShape(background_rect(id, bg, page)));
        id += 1;
    }

    for item in &page.items {
        let shape = match item {
            Item::Shape(s) => Shape::AutoShape(build_auto_shape(id, s)),
            Item::Image(img) => {
                let format = match img.format {
                    ImageFormat::Png => PictureFormat::Png,
                    ImageFormat::Jpeg => PictureFormat::Jpeg,
                    ImageFormat::Gif => PictureFormat::Gif,
                };
                Shape::Picture(build_picture(
                    id,
                    &img.name,
                    img.data.clone(),
                    format,
                    img.frame,
                ))
            }
            Item::Raster(r) => Shape::Picture(build_picture(
                id,
                &r.name,
                r.png.clone(),
                PictureFormat::Png,
                r.frame,
            )),
            Item::Text(t) => Shape::AutoShape(build_text_box(id, t)),
        };
        slide.shapes.push(shape);
        id += 1;
    }
    slide
}

fn build_picture(
    id: u32,
    name: &str,
    data: Vec<u8>,
    format: PictureFormat,
    frame: crate::ir::Frame,
) -> Picture {
    Picture {
        id,
        name: name.to_string(),
        data,
        format,
        offset_emu: (frame.x, frame.y),
        extent_emu: (frame.w.max(1), frame.h.max(1)),
        description: String::new(),
        shape_properties: None,
        external_link: None,
    }
}

/// 铺满画布的背景矩形（DrawingML 的 slide background 该库未建模，用底层矩形等价实现）。
fn background_rect(id: u32, color: crate::geom::Rgb, page: &Page) -> AutoShape {
    let (w, h) = (page.width_emu, page.height_emu);
    let mut shape = AutoShape::new(id, "背景");
    shape.properties = ShapeProperties::new()
        .with_transform(Transform2D {
            offset: Some((0, 0)),
            extent: Some((w, h)),
            rotation_60000ths: 0,
            flip_horizontal: false,
            flip_vertical: false,
        })
        .with_geometry(Geometry::Custom(CustomGeometry {
            width_emu: w,
            height_emu: h,
            commands: vec![
                PathCommand::MoveTo { x: 0, y: 0 },
                PathCommand::LineTo { x: w, y: 0 },
                PathCommand::LineTo { x: w, y: h },
                PathCommand::LineTo { x: 0, y: h },
                PathCommand::Close,
            ],
        }))
        .with_fill(Fill::Solid(Color::Rgb(color.hex())));
    shape
}

fn build_auto_shape(id: u32, item: &crate::ir::ShapeItem) -> AutoShape {
    let mut shape = AutoShape::new(id, item.name.clone());
    let frame = item.frame;

    let mut properties = ShapeProperties::new()
        .with_transform(Transform2D {
            offset: Some((frame.x, frame.y)),
            extent: Some((frame.w.max(1), frame.h.max(1))),
            rotation_60000ths: 0,
            flip_horizontal: false,
            flip_vertical: false,
        })
        .with_geometry(Geometry::Custom(CustomGeometry {
            width_emu: frame.w.max(1),
            height_emu: frame.h.max(1),
            commands: item.commands.iter().map(convert_cmd).collect(),
        }));

    if let Some(fill) = &item.fill {
        properties = properties.with_fill(convert_fill(fill));
    } else {
        properties = properties.with_fill(Fill::None);
    }
    if let Some(stroke) = &item.stroke {
        properties = properties.with_line(convert_stroke(stroke));
    }

    shape.properties = properties;
    shape
}

/// 可编辑文本框：`prstGeom` 形状壳 + `<p:txBody>`，并用 `txBox="1"` 标记成真文本框。
fn build_text_box(id: u32, item: &TextItem) -> AutoShape {
    let frame = item.frame;
    let (w, h) = (frame.w.max(1), frame.h.max(1));

    // 文本框自身不画边框、不填底色，只承载文字。
    let properties = ShapeProperties::new()
        .with_transform(Transform2D {
            offset: Some((frame.x, frame.y)),
            extent: Some((w, h)),
            rotation_60000ths: 0,
            flip_horizontal: false,
            flip_vertical: false,
        })
        .with_geometry(Geometry::Custom(CustomGeometry {
            width_emu: w,
            height_emu: h,
            commands: vec![
                PathCommand::MoveTo { x: 0, y: 0 },
                PathCommand::LineTo { x: w, y: 0 },
                PathCommand::LineTo { x: w, y: h },
                PathCommand::LineTo { x: 0, y: h },
                PathCommand::Close,
            ],
        }))
        .with_fill(Fill::None);

    // 四个内边距显式置 0，否则 PowerPoint 会塞进默认的 0.1 英寸留白，文字整体偏移。
    let body_properties = TextBodyProperties {
        wrap: Some(TextWrap::None),
        anchor: Some(TextAnchor::Top),
        anchor_center: false,
        inset_left_emu: Some(0),
        inset_top_emu: Some(0),
        inset_right_emu: Some(0),
        inset_bottom_emu: Some(0),
        autofit: Some(TextAutofit::None),
        vertical_direction: None,
        rotation_60000ths: 0,
    };

    let mut paragraph = TextParagraph::new().with_properties(
        TextParagraphProperties::new().with_alignment(match item.align {
            TextHAlign::Left => TextAlign::Left,
            TextHAlign::Center => TextAlign::Center,
            TextHAlign::Right => TextAlign::Right,
        }),
    );
    for run in &item.runs {
        paragraph = paragraph.with_run(TextRun::Regular {
            text: run.text.clone(),
            properties: TextRunProperties {
                bold: run.bold,
                italic: run.italic,
                underline: if run.underline {
                    Some(TextUnderline::Single)
                } else {
                    None
                },
                strike: if run.strike {
                    Some(TextStrike::Single)
                } else {
                    None
                },
                fill: Some(Fill::Solid(Color::Rgb(run.color.hex()))),
                font_size_100ths_point: Some(run.size_100ths_pt),
                font_family: run.font_family.clone(),
                hyperlink: None,
                baseline_1000ths_percent: None,
                highlight: None,
                text_caps: None,
                character_spacing_100ths_point: None,
            },
        });
    }

    AutoShape::new(id, item.name.clone())
        .with_properties(properties)
        .with_text_body(
            TextBody::new()
                .with_properties(body_properties)
                .with_paragraph(paragraph),
        )
        .with_text_box(true)
}

fn convert_cmd(cmd: &PathCmd) -> PathCommand {
    match *cmd {
        PathCmd::MoveTo(x, y) => PathCommand::MoveTo { x, y },
        PathCmd::LineTo(x, y) => PathCommand::LineTo { x, y },
        PathCmd::CubicTo(x1, y1, x2, y2, x, y) => PathCommand::CubicBezierTo {
            x1,
            y1,
            x2,
            y2,
            x,
            y,
        },
        PathCmd::Close => PathCommand::Close,
    }
}

fn convert_fill(paint: &Paint) -> Fill {
    match paint {
        Paint::None => Fill::None,
        Paint::Solid(rgb) => Fill::Solid(Color::Rgb(rgb.hex())),
        Paint::LinearGradient { angle_deg, stops } => {
            let mut gs: Vec<GradientStop> = stops
                .iter()
                .map(|s| GradientStop::new(s.pos * 100.0, Color::Rgb(s.color.hex())))
                .collect();
            // 线性渐变至少要有两个停靠点，否则 PowerPoint 会判为无效内容。
            if gs.len() == 1 {
                let only = gs[0].clone();
                gs.push(only);
            }
            if gs.is_empty() {
                return Fill::None;
            }
            Fill::Gradient(GradientFill {
                stops: gs,
                angle_60000ths: Some(degrees_to_units(*angle_deg)),
            })
        }
    }
}

fn convert_stroke(stroke: &Stroke) -> Line {
    Line {
        width_emu: Some(stroke.width_emu.max(1)),
        fill: Some(convert_fill(&stroke.paint)),
        dash: stroke.dash.map(|d| match d {
            DashKind::Dot => PresetLineDash::Dot,
            DashKind::Dash => PresetLineDash::Dash,
            DashKind::LargeDash => PresetLineDash::LargeDash,
            DashKind::DashDot => PresetLineDash::DashDot,
            DashKind::LargeDashDot => PresetLineDash::LargeDashDot,
            DashKind::LargeDashDotDot => PresetLineDash::LargeDashDotDot,
        }),
        cap: Some(match stroke.cap {
            CapKind::Butt => LineCap::Flat,
            CapKind::Round => LineCap::Round,
            CapKind::Square => LineCap::Square,
        }),
        join: Some(match stroke.join {
            JoinKind::Miter => LineJoin::Miter {
                limit_1000ths_percent: None,
            },
            JoinKind::Round => LineJoin::Round,
            JoinKind::Bevel => LineJoin::Bevel,
        }),
        head_end: None,
        tail_end: None,
        compound: None,
    }
}
