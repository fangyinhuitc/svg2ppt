//! 错误类型。

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("无法解析 SVG：{0}")]
    Svg(#[from] usvg::Error),

    #[error("系统里没有可用字体，请用 --font-dir 指定字体目录（服务端部署时很常见）")]
    NoFonts,

    #[error("SVG 尺寸为 0，无法转换")]
    EmptySvg,

    #[error("光栅化失败：{0}")]
    Raster(String),

    #[error("写入 PPTX 失败：{0}")]
    Pptx(String),

    #[error("IO 错误：{0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}
