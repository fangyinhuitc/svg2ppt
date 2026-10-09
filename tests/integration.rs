//! 端到端测试：SVG → PPTX 字节 → 解开 OPC 包检查内容。

use std::io::{Cursor, Read};

use svg2ppt::{ConvertOptions, Mode, Rgb, TextMode, convert, convert_many};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn slide_xml(bytes: &[u8], index: usize) -> String {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).unwrap();
    let name = format!("ppt/slides/slide{index}.xml");
    let mut s = String::new();
    zip.by_name(&name)
        .unwrap_or_else(|e| panic!("缺少 {name}: {e}"))
        .read_to_string(&mut s)
        .unwrap();
    s
}

fn entries(bytes: &[u8]) -> Vec<String> {
    let zip = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).unwrap();
    zip.file_names()
        .filter_map(|s| s.ok())
        .map(|s| s.to_string())
        .collect()
}

#[test]
fn vector_mode_emits_custom_geometry() {
    let opts = ConvertOptions {
        text_mode: TextMode::Path,
        ..Default::default()
    };
    let (bytes, report) = convert(&fixture("basic.svg"), &opts).unwrap();
    assert_eq!(&bytes[0..2], b"PK", "应当是一个 zip 包");

    let xml = slide_xml(&bytes, 1);
    // 每个 SVG 形状都应该变成一段 custGeom 路径。
    assert!(
        xml.matches("<a:custGeom>").count() >= 6,
        "形状数不足：{xml:.200}"
    );
    assert!(xml.contains("<a:solidFill>"));
    assert!(xml.contains("<a:gradFill>"), "线性渐变应被保留");
    assert!(xml.contains("<a:ln "), "描边应被保留");

    // 显式要求 path 模式时，中文文本转成字形轮廓，会产生大量贝塞尔段。
    assert!(xml.contains("<a:cubicBezTo"), "文本应转成字形轮廓");
    assert!(!xml.contains("txBox=\"1\""), "path 模式不该产生文本框");
    assert!(
        report.diagnostics.iter().any(|d| d.code == "text-as-path"),
        "应记录文本转曲的诊断"
    );
}

#[test]
fn auto_text_mode_emits_editable_text_box() {
    let (bytes, report) = convert(&fixture("basic.svg"), &ConvertOptions::default()).unwrap();
    let xml = slide_xml(&bytes, 1);

    assert!(xml.contains("<p:txBody>"), "应产生文本框");
    assert!(xml.contains("txBox=\"1\""), "应标记为真文本框");
    assert!(
        xml.contains("<a:t>中文标题与 English Text</a:t>"),
        "文字应可编辑（原样保留）"
    );
    assert!(
        xml.contains("<a:latin typeface=\"PingFang SC\"/>"),
        "应带上字体族"
    );
    // 28 px 的字体在 640 px 宽、13.33 英寸的画布上等于 42 pt。
    assert!(xml.contains("sz=\"4200\""), "字号应按画布缩放换算成磅值");
    assert!(
        !report.diagnostics.iter().any(|d| d.code == "text-as-path"),
        "默认模式下这段文本不该转曲：{:?}",
        report.diagnostics
    );
}

#[test]
fn complex_text_falls_back_to_outlines_with_reasons() {
    let (bytes, report) = convert(&fixture("text.svg"), &ConvertOptions::default()).unwrap();
    let codes: Vec<_> = report.diagnostics.iter().map(|d| d.code).collect();
    assert!(codes.contains(&"text-multiline-fallback"), "{codes:?}");
    assert!(codes.contains(&"text-rotated-fallback"), "{codes:?}");
    assert!(codes.contains(&"text-spacing-fallback"), "{codes:?}");

    let xml = slide_xml(&bytes, 1);
    // 只有第一条（单行、居中、两个 run）走文本框，其余三条转曲。
    assert_eq!(xml.matches("txBox=\"1\"").count(), 1);
    let block = {
        let start = xml.find("<p:txBody>").unwrap();
        xml[start..xml[start..].find("</p:txBody>").unwrap() + start].to_string()
    };
    assert!(
        block.contains("<a:t>红色</a:t>") && block.contains("<a:t>与蓝色</a:t>"),
        "{block}"
    );
    assert!(
        block.contains("algn=\"ctr\""),
        "text-anchor=middle 应映射成居中"
    );
    assert!(xml.contains("<a:cubicBezTo"), "回落的三段文本应转成轮廓");
}

#[test]
fn image_lands_at_its_svg_position() {
    let (bytes, _) = convert(&fixture("image.svg"), &ConvertOptions::default()).unwrap();
    let names = entries(&bytes);
    assert!(
        names.iter().any(|n| n == "ppt/media/image1.png"),
        "{names:?}"
    );

    let xml = slide_xml(&bytes, 1);
    let pic = {
        let start = xml.find("<p:pic>").unwrap();
        xml[start..xml[start..].find("</p:pic>").unwrap() + start].to_string()
    };
    // 400×200 的 viewBox 放进 16:9 画布：scale = 12192000/400 = 30480 EMU/px，
    // 高度居中带来 381000 EMU 的纵向留白。
    assert!(
        pic.contains("<a:off x=\"1219200\" y=\"2209800\"/>"),
        "{pic}"
    );
    assert!(
        pic.contains("<a:ext cx=\"2438400\" cy=\"2438400\"/>"),
        "{pic}"
    );
}

#[test]
fn filter_and_radial_fall_back_to_raster() {
    let (bytes, report) = convert(&fixture("filtered.svg"), &ConvertOptions::default()).unwrap();
    let codes: Vec<_> = report.diagnostics.iter().map(|d| d.code).collect();
    assert!(codes.contains(&"filter-rasterized"), "{codes:?}");
    assert!(codes.contains(&"radial-gradient-rasterized"), "{codes:?}");

    // 降级产物应当作为图片部件真的写进了包。
    let names = entries(&bytes);
    assert!(
        names.iter().any(|n| n.starts_with("ppt/media/")),
        "降级后应有媒体部件：{names:?}"
    );

    let rels = {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes.clone())).unwrap();
        let mut s = String::new();
        zip.by_name("ppt/slides/_rels/slide1.xml.rels")
            .unwrap()
            .read_to_string(&mut s)
            .unwrap();
        s
    };
    assert!(rels.contains("image"), "slide 关系里应有图片引用");
}

#[test]
fn raster_mode_embeds_one_full_page_picture() {
    let opts = ConvertOptions {
        mode: Mode::Raster,
        ..Default::default()
    };
    let (bytes, report) = convert(&fixture("basic.svg"), &opts).unwrap();
    assert!(
        report.is_empty(),
        "raster 模式不该有降级：{:?}",
        report.diagnostics
    );

    let xml = slide_xml(&bytes, 1);
    assert_eq!(xml.matches("<p:pic>").count(), 1);
    assert_eq!(xml.matches("<a:custGeom>").count(), 0);
}

#[test]
fn multiple_inputs_become_multiple_slides() {
    let a = fixture("basic.svg");
    let b = fixture("filtered.svg");
    let (bytes, _) =
        convert_many(&[a.as_slice(), b.as_slice()], &ConvertOptions::default()).unwrap();
    let names = entries(&bytes);
    assert!(names.contains(&"ppt/slides/slide1.xml".to_string()));
    assert!(names.contains(&"ppt/slides/slide2.xml".to_string()));
}

#[test]
fn background_option_adds_base_rectangle() {
    let opts = ConvertOptions {
        background: Some(Rgb::WHITE),
        ..Default::default()
    };
    let (bytes, _) = convert(&fixture("basic.svg"), &opts).unwrap();
    let xml = slide_xml(&bytes, 1);
    assert!(xml.contains("FFFFFF"), "背景色应在幻灯片的第一个形状里");
}

#[test]
fn half_transparent_fill_is_blended_not_dropped() {
    // basic.svg 里的圆是 #F59E0B + fill-opacity 0.6，混白后应变成 #F9C56D 附近。
    let (bytes, _) = convert(&fixture("basic.svg"), &ConvertOptions::default()).unwrap();
    let xml = slide_xml(&bytes, 1);
    assert!(xml.contains("F9C56D"), "半透明填充应被预乘为接近的不透明色");
}

#[test]
fn invalid_input_reports_error() {
    let err = convert(b"<svg>", &ConvertOptions::default());
    assert!(err.is_err());
}

/// 一条 2000 点的密集折线，用来验证简化真的在削顶点。
fn dense_polyline_svg() -> Vec<u8> {
    let n = 2000;
    let mut d = String::from("M ");
    for i in 0..n {
        let x = 20.0 + 600.0 * i as f64 / (n - 1) as f64;
        let y = 180.0 - 120.0 * (6.0 * std::f64::consts::PI * i as f64 / (n - 1) as f64).sin();
        d.push_str(&format!("{x:.3} {y:.3} "));
        if i + 1 < n {
            d.push('L');
        }
    }
    // 注意用 r##"…"##：路径里的颜色值 `"#2563eb"` 含 `"#`，会提前终止 r#"…"#
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 640 360" width="640" height="360">
<path d="{d}" fill="none" stroke="#2563eb" stroke-width="2"/></svg>"##
    )
    .into_bytes()
}

#[test]
fn simplify_cuts_points_and_is_reported() {
    let svg = dense_polyline_svg();
    let opts = |s: f64| ConvertOptions {
        simplify: s,
        ..Default::default()
    };

    let (raw_bytes, raw_report) = convert(&svg, &opts(0.0)).unwrap();
    assert_eq!(raw_report.simplified_points, 0, "关闭简化时不该有删点统计");

    let (sim_bytes, sim_report) = convert(&svg, &opts(0.2)).unwrap();
    assert!(
        sim_report.simplified_points > 1000,
        "密集折线应删掉大量顶点，实际 {}",
        sim_report.simplified_points
    );
    assert!(sim_bytes.len() * 2 < raw_bytes.len(), "产物体积应显著下降");

    let n_raw = slide_xml(&raw_bytes, 1).matches("<a:lnTo").count();
    let n_sim = slide_xml(&sim_bytes, 1).matches("<a:lnTo").count();
    assert!(n_sim * 2 < n_raw, "折线指令数应减半以上：{n_raw} → {n_sim}");
    // 起点与终点是形状的骨架，绝不能删。
    assert!(n_sim >= 2);
}

#[test]
fn simplify_never_touches_curve_segments() {
    // 贝塞尔的控制点不是路径上的点，删任何一个都会改变曲线形状 —— 简化必须放过它们。
    let opts = |s: f64| ConvertOptions {
        simplify: s,
        text_mode: TextMode::Path,
        ..Default::default()
    };
    let (a, _) = convert(&fixture("golden-geom.svg"), &opts(0.0)).unwrap();
    let (b, _) = convert(&fixture("golden-geom.svg"), &opts(5.0)).unwrap();
    assert_eq!(
        slide_xml(&a, 1).matches("<a:cubicBezTo").count(),
        slide_xml(&b, 1).matches("<a:cubicBezTo").count(),
        "曲线段的数量不该受简化影响"
    );
}
