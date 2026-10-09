//! 调试用：打印转换后的 IR（跳过 PPTX 写入），便于定位几何问题。

use svg2ppt::convert::{ConvertOptions, svg_to_page};
use svg2ppt::ir::Item;

fn main() {
    let path = std::env::args().nth(1).expect("usage: ir <file.svg>");
    let data = std::fs::read(&path).unwrap();
    let opts = ConvertOptions::default();
    let (page, report) = svg_to_page(&data, &opts).unwrap();
    println!("canvas: {}x{} EMU", page.width_emu, page.height_emu);
    for item in &page.items {
        match item {
            Item::Shape(s) => {
                println!(
                    "Shape {:>6}  frame=({}, {}, {}, {})  cmds={}  fill={:?}",
                    s.name,
                    s.frame.x,
                    s.frame.y,
                    s.frame.w,
                    s.frame.h,
                    s.commands.len(),
                    s.fill.as_ref().map(|f| match f {
                        svg2ppt::ir::Paint::Solid(c) => format!("solid {}", c.hex()),
                        svg2ppt::ir::Paint::None => "none".to_string(),
                        svg2ppt::ir::Paint::LinearGradient { angle_deg, stops } => {
                            format!("gradient {}deg {}stops", angle_deg, stops.len())
                        }
                    })
                );
                for c in s.commands.iter().take(4) {
                    println!("        {:?}", c);
                }
            }
            Item::Image(i) => println!("Image  {:>6}  frame=({:?})", i.name, i.frame),
            Item::Raster(r) => println!(
                "Raster {:>6}  frame=({:?}) png={}B",
                r.name,
                r.frame,
                r.png.len()
            ),
            Item::Text(t) => {
                println!(
                    "Text   {:>6}  frame=({}, {}, {}, {})  align={:?}  runs={}",
                    t.name,
                    t.frame.x,
                    t.frame.y,
                    t.frame.w,
                    t.frame.h,
                    t.align,
                    t.runs.len()
                );
                for r in &t.runs {
                    println!(
                        "        {:?} size={}pt bold={} italic={} color={}",
                        r.text,
                        r.size_100ths_pt as f64 / 100.0,
                        r.bold,
                        r.italic,
                        r.color.hex()
                    );
                }
            }
        }
    }
    for (code, n) in report.summary() {
        println!("diag: {code} x{n}");
    }
}
