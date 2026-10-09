# svg2ppt

把 SVG 转成 `.pptx` 的命令行工具与库。纯 Rust，无外部二进制依赖。

设计文档见 [`docs/design.md`](docs/design.md)（含技术选型对比、SVG→DrawingML 映射规则、保真度降级矩阵）。

## 为什么不是「截图贴图」

默认的 `vector` 模式把 SVG 变成**真实的 DrawingML 形状**（`a:custGeom` 路径），在 PowerPoint 里可以选中、改色、改描边、缩放不失真。只有 DrawingML 表达不了的东西（滤镜、遮罩、径向渐变、图案填充…）才会局部降级为位图，而且**每一处降级都会报告出来**，不会静默丢东西。

文字也一样：默认的 `--text-mode auto` 对**单行、无旋转、无字距调整**的文本直接输出 PPT 文本框，对方可以在 PowerPoint 里放光标改字；只有 PPT 排版引擎一定会重排走样的文本（多行、旋转、字距、上下标…）才转成字形轮廓——转曲原因会打印成诊断码。想要 100% 像素保真、不在乎可编辑性就用 `--text-mode path`。

## 用法

```bash
# 单个文件，默认矢量模式
svg2ppt diagram.svg -o diagram.pptx

# 多个 SVG → 一个 pptx（每份一页）
svg2ppt step1.svg step2.svg step3.svg -o flow.pptx

# 每个文件各自输出
svg2ppt *.svg --per-file

# 位图模式（100% 视觉保真，但不可编辑）
svg2ppt art.svg --mode raster --scale 3 -o art.pptx

# 4:3 画布 + 白底 + 铺满
svg2ppt chart.svg --size 4:3 --background white --fit cover

# 文字一律转曲（像素保真优先，代价是不可编辑）
svg2ppt poster.svg --text-mode path

# CI 守门：出现任何降级就失败（退出码 2）
svg2ppt diagram.svg --strict
```

## 参数

| 参数 | 说明 |
| --- | --- |
| `--mode vector\|raster` | `vector`（默认）输出可编辑形状；`raster` 整页转位图 |
| `--size 16:9\|4:3\|WxH` | 画布尺寸，`WxH` 以 px 计，如 `1920x1080` |
| `--fit contain\|cover\|stretch\|none` | 适配方式，默认 `contain` |
| `--background <COLOR>` | 背景色，同时作为半透明像素的混色基底 |
| `--text-mode auto\|path` | `auto`（默认）能安全表达的单行文本写成**可编辑文本框**，其余转曲；`path` 一律转曲（像素级保真） |
| `--scale <F>` | 位图超采样倍率，默认 2.0 |
| `--font-dir <DIR>` | 额外字体目录（服务端部署常用） |
| `--per-file` | 每个输入单独输出 |
| `--strict` | 有降级即失败 |
| `-q, --quiet` | 不打印降级摘要 |

## 作为库使用

```rust
use svg2ppt::{convert, ConvertOptions};

let svg = std::fs::read("diagram.svg")?;
let (pptx, report) = convert(&svg, &ConvertOptions::default())?;
std::fs::write("diagram.pptx", pptx)?;

for (code, n) in report.summary() {
    eprintln!("降级：{code} ×{n}");
}
```

## 降级码一览

| 码 | 含义 |
| --- | --- |
| `text-as-path` | 文本转曲（`--text-mode path`，保真但不可编辑） |
| `text-multiline-fallback` | 多行文本无法交给 PPT 精确重排 → 转曲 |
| `text-rotated-fallback` / `text-per-glyph-offset-fallback` | 旋转或逐字形位移 → 转曲 |
| `text-spacing-fallback` / `text-length-adjust-fallback` | 字距 / `textLength` 调整 → 转曲 |
| `text-on-path-fallback` / `text-vertical-fallback` | 路径文字 / 竖排 → 转曲 |
| `text-stroked-fallback` / `text-paint-fallback` | 描边文字 / 非纯色填充 → 转曲 |
| `text-baseline-shift-fallback` / `text-small-caps-fallback` / `text-hidden-fallback` | 上下标 / small-caps / 隐藏文本 → 转曲 |
| `text-empty-fallback` | 文本为空 → 转曲 |
| `alpha-approximated` | 半透明与背景色预乘 |
| `group-opacity-flattened` | 分组透明度近似到子元素 |
| `filter-rasterized` / `clip-path-rasterized` / `mask-rasterized` | 滤镜 / 裁剪 / 遮罩 → 局部位图 |
| `radial-gradient-rasterized` / `pattern-rasterized` | 径向渐变 / 图案填充 → 局部位图 |
| `rotated-image-rasterized` | 带旋转的图片 → 局部位图 |
| `unsupported-image-rasterized` | WebP / 嵌套 SVG 图片 → 局部位图 |
| `dash-approximated` | 自定义虚线就近映射为预设虚线 |
| `gradient-spread-approximated` | 渐变 reflect/repeat 按 pad 处理 |

## 开发

```bash
cargo test          # 18 项：单元 + 集成 + doctest
cargo clippy --all-targets
cargo run --example ir -- tests/fixtures/basic.svg   # 看转换后的中间表示
cargo run --example inspect -- tests/fixtures/basic.svg  # 看 usvg 的节点变换
```
