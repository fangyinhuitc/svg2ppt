//! SVG 解析：把原始字节交给 usvg，得到一棵已经「归一化」的树。

use std::path::PathBuf;
use std::sync::Arc;

use crate::error::Error;

/// 解析 SVG（自动识别 SVGZ）。
///
/// usvg 会在这一步把 CSS、`use`、引用、相对单位、marker 等全部解析成最简单的形式，
/// 之后我们只需要面对 `Path` / `Image` / `Text` 三类叶子节点。
pub fn parse(data: &[u8], font_dirs: &[PathBuf]) -> Result<usvg::Tree, Error> {
    let mut fontdb = usvg::fontdb::Database::new();
    fontdb.load_system_fonts();
    for dir in font_dirs {
        fontdb.load_fonts_dir(dir);
    }
    if fontdb.is_empty() {
        return Err(Error::NoFonts);
    }

    let options = usvg::Options {
        fontdb: Arc::new(fontdb),
        ..Default::default()
    };

    let tree = usvg::Tree::from_data(data, &options)?;

    let size = tree.size();
    if size.width() <= 0.0 || size.height() <= 0.0 {
        return Err(Error::EmptySvg);
    }
    Ok(tree)
}
