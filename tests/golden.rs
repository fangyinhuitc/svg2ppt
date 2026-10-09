//! 金样（snapshot）测试：锁住 emit 层实际写出的 DrawingML。
//!
//! 两条设计约束，改动前请先读：
//!
//! 1. **不能对整个 pptx 做哈希**。`office-toolkit` 会把当前时间写进 zip 条目的
//!    DOS 时间戳，同一输入两次运行字节也不同（已实测）。所以比对的是
//!    `ppt/slides/slideN.xml` 的内容 —— 它不含任何时间或随机量，是确定的。
//! 2. **金样 fixture 不能含文本**。文本走字形轮廓，而轮廓来自系统字体，
//!    macOS / Windows / Linux 的字体不同，快照必然跨平台失败。
//!    字体相关的回归由 `integration.rs` 的行为断言覆盖（只断言性质，不比对数值）。
//!
//! 更新快照：`SVG2PPT_UPDATE_GOLDEN=1 cargo test --test golden`

use std::io::{Cursor, Read};
use std::path::PathBuf;

use svg2ppt::{ConvertOptions, TextMode, convert};

/// 纳入金样的 fixture。新增时请同时遵守上面第 2 条（不含文本）。
const GOLDEN_FIXTURES: &[&str] = &["golden-geom.svg"];

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("读不到 {path:?}：{e}"))
}

fn slide_xml(bytes: &[u8], index: usize) -> String {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).unwrap();
    let name = format!("ppt/slides/slide{index}.xml");
    let mut s = String::new();
    zip.by_name(&name)
        .unwrap_or_else(|e| panic!("缺少 {name}：{e}"))
        .read_to_string(&mut s)
        .unwrap();
    s
}

/// 在标签边界断行：内容不变，但 diff 可读。
fn normalize(xml: &str) -> String {
    xml.replace("><", ">\n<")
}

fn snapshot_path(fixture_name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
        .join(format!("{fixture_name}.txt"))
}

#[test]
fn drawingml_matches_golden() {
    let update = std::env::var("SVG2PPT_UPDATE_GOLDEN").as_deref() == Ok("1");

    for name in GOLDEN_FIXTURES {
        let opts = ConvertOptions {
            text_mode: TextMode::Path,
            ..Default::default()
        };
        let (bytes, _) = convert(&fixture(name), &opts).unwrap();
        let actual = normalize(&slide_xml(&bytes, 1));
        let path = snapshot_path(name);

        if update {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).unwrap();
            }
            std::fs::write(&path, &actual).unwrap();
            println!("已更新金样：{}", path.display());
            continue;
        }

        let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| {
            panic!(
                "缺少金样快照：{}\n用 `SVG2PPT_UPDATE_GOLDEN=1 cargo test --test golden` 生成，并确认 diff 是你预期的改动",
                path.display()
            )
        });
        assert_eq!(
            actual, expected,
            "金样不匹配：{name}\n若改动是有意的，用 `SVG2PPT_UPDATE_GOLDEN=1 cargo test --test golden` 更新"
        );
    }
}

#[test]
fn slide_xml_is_deterministic() {
    // 金样的前提：同一输入两次转换必须产出完全相同的 slide XML。
    // 这条测试挂了，说明有非确定性进入了 emit 层（时间戳、随机 id、HashMap 迭代顺序…）。
    let opts = ConvertOptions {
        text_mode: TextMode::Path,
        ..Default::default()
    };
    let input = fixture("golden-geom.svg");
    let (a, _) = convert(&input, &opts).unwrap();
    let (b, _) = convert(&input, &opts).unwrap();
    assert_eq!(
        slide_xml(&a, 1),
        slide_xml(&b, 1),
        "同输入两次转换的 slide XML 不一致，金样将无法稳定"
    );
}
