//! svg2ppt —— 把 SVG 转换成 PPTX。
//!
//! ```no_run
//! use svg2ppt::{convert, ConvertOptions};
//!
//! let svg = std::fs::read("diagram.svg").unwrap();
//! let (pptx, report) = convert(&svg, &ConvertOptions::default()).unwrap();
//! std::fs::write("diagram.pptx", pptx).unwrap();
//! if !report.is_empty() {
//!     for (code, n) in report.summary() {
//!         eprintln!("{code} ×{n}");
//!     }
//! }
//! ```

pub mod convert;
pub mod emit;
pub mod error;
pub mod geom;
pub mod ir;
pub mod raster;
pub mod svg;

pub use convert::{ConvertOptions, Mode, SlideSize, TextMode};
pub use error::Error;
pub use geom::{Fit, Rgb};
pub use ir::{Diagnostic, Report};

/// 把一份 SVG 转成 pptx 字节。
pub fn convert(svg: &[u8], opts: &ConvertOptions) -> Result<(Vec<u8>, Report), Error> {
    let (page, report) = convert::svg_to_page(svg, opts)?;
    let bytes = emit::build_pptx(std::slice::from_ref(&page))?;
    Ok((bytes, report))
}

/// 把多份 SVG 合并成一个 pptx（每份一页）。
pub fn convert_many(svgs: &[&[u8]], opts: &ConvertOptions) -> Result<(Vec<u8>, Report), Error> {
    let mut pages = Vec::with_capacity(svgs.len());
    let mut report = Report::default();
    for svg in svgs {
        let (page, r) = convert::svg_to_page(svg, opts)?;
        pages.push(page);
        report.extend(r);
    }
    let bytes = emit::build_pptx(&pages)?;
    Ok((bytes, report))
}
