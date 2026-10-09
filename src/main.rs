//! svg2ppt 命令行入口。

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, bail};
use clap::{Parser, ValueEnum};

use svg2ppt::{ConvertOptions, DEFAULT_SIMPLIFY_PX, Fit, Mode, Rgb, SlideSize, TextMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ModeArg {
    /// 转成可编辑的 DrawingML 形状（默认）
    Vector,
    /// 整页渲染成位图，保真优先
    Raster,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum FitArg {
    /// 等比缩放并居中，留白（默认）
    Contain,
    /// 等比缩放并居中，裁掉溢出部分
    Cover,
    /// 非等比拉伸铺满
    Stretch,
    /// 1:1，不缩放
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum TextModeArg {
    /// 混合：能安全表达的用 PPT 文本框（可编辑），其余回落转曲（默认）
    Auto,
    /// 全部转成字形轮廓：像素级保真，但不可编辑
    Path,
}

impl From<TextModeArg> for TextMode {
    fn from(v: TextModeArg) -> Self {
        match v {
            TextModeArg::Auto => TextMode::Auto,
            TextModeArg::Path => TextMode::Path,
        }
    }
}

impl From<FitArg> for Fit {
    fn from(v: FitArg) -> Self {
        match v {
            FitArg::Contain => Fit::Contain,
            FitArg::Cover => Fit::Cover,
            FitArg::Stretch => Fit::Stretch,
            FitArg::None => Fit::None,
        }
    }
}

#[derive(Parser, Debug)]
#[command(name = "svg2ppt", version, about = "把 SVG 转换成 .pptx")]
struct Cli {
    /// 输入 SVG 文件（可多个；默认合并为一个 pptx 的连续页）
    #[arg(value_name = "INPUT", required = true)]
    inputs: Vec<PathBuf>,

    /// 输出路径（默认取第一个输入的主文件名 + .pptx）
    #[arg(short, long, value_name = "FILE")]
    output: Option<PathBuf>,

    /// 输出模式
    #[arg(long, value_enum, default_value = "vector")]
    mode: ModeArg,

    /// 画布尺寸：16:9 / 4:3 / WxH（如 1920x1080，单位 px）
    #[arg(long, default_value = "16:9")]
    size: String,

    /// 适配方式
    #[arg(long, value_enum, default_value = "contain")]
    fit: FitArg,

    /// 背景色（同时作为半透明像素的混色基底），如 #ffffff / white
    #[arg(long, value_name = "COLOR")]
    background: Option<String>,

    /// 文本处理方式
    #[arg(long, value_enum, default_value = "auto")]
    text_mode: TextModeArg,

    /// 位图超采样倍率（相对画布逻辑像素）
    #[arg(long, default_value_t = 2.0)]
    scale: f64,

    /// 折线简化容差（SVG px）：删掉对形状无可见贡献的折线顶点以压住体积，0 表示关闭
    #[arg(long, default_value_t = DEFAULT_SIMPLIFY_PX, value_name = "PX")]
    simplify: f64,

    /// 额外字体目录，可重复指定
    #[arg(long, value_name = "DIR")]
    font_dir: Vec<PathBuf>,

    /// 每个输入单独输出一个 pptx
    #[arg(long)]
    per_file: bool,

    /// 只要出现任何降级就以退出码 2 失败（适合 CI 守门）
    #[arg(long)]
    strict: bool,

    /// 不打印降级摘要
    #[arg(short, long)]
    quiet: bool,
}

fn parse_size(s: &str) -> anyhow::Result<SlideSize> {
    let s = s.trim().to_lowercase();
    match s.as_str() {
        "16:9" | "16x9" | "widescreen" => return Ok(SlideSize::Widescreen16x9),
        "4:3" | "4x3" | "onscreen" => return Ok(SlideSize::OnScreen4x3),
        _ => {}
    }
    let (w, h) = s.split_once(['x', 'X', '*']).with_context(|| {
        format!("无法解析画布尺寸 `{s}`，请用 16:9 / 4:3 / 1920x1080 这样的形式")
    })?;
    let w: f64 = w.trim().parse().context("宽度不是数字")?;
    let h: f64 = h.trim().parse().context("高度不是数字")?;
    if w <= 0.0 || h <= 0.0 {
        bail!("画布尺寸必须为正数");
    }
    Ok(SlideSize::Custom {
        width_emu: (w * 9525.0).round() as i64,
        height_emu: (h * 9525.0).round() as i64,
    })
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("错误：{e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let background = match &cli.background {
        Some(s) => Some(Rgb::parse(s).with_context(|| format!("无法解析颜色 `{s}`"))?),
        None => None,
    };

    let opts = ConvertOptions {
        mode: match cli.mode {
            ModeArg::Vector => Mode::Vector,
            ModeArg::Raster => Mode::Raster,
        },
        slide: parse_size(&cli.size)?,
        fit: cli.fit.into(),
        background,
        text_mode: cli.text_mode.into(),
        scale: cli.scale.max(0.1),
        simplify: cli.simplify.max(0.0),
        font_dirs: cli.font_dir.clone(),
    };

    let inputs = cli
        .inputs
        .iter()
        .map(|p| {
            Ok((
                p.clone(),
                std::fs::read(p).with_context(|| format!("读取 {} 失败", p.display()))?,
            ))
        })
        .collect::<anyhow::Result<Vec<(PathBuf, Vec<u8>)>>>()?;

    if cli.per_file {
        for (path, data) in &inputs {
            let out = cli
                .output
                .clone()
                .unwrap_or_else(|| path.with_extension("pptx"));
            let (bytes, report) = svg2ppt::convert(data, &opts)?;
            std::fs::write(&out, &bytes).with_context(|| format!("写入 {} 失败", out.display()))?;
            print_report(&out, &report, cli.quiet);
            if cli.strict && !report.is_empty() {
                eprintln!("--strict：{} 存在降级，视为失败", out.display());
                std::process::exit(2);
            }
        }
        return Ok(());
    }

    let out = cli
        .output
        .clone()
        .unwrap_or_else(|| inputs[0].0.with_extension("pptx"));
    let refs: Vec<&[u8]> = inputs.iter().map(|(_, d)| d.as_slice()).collect();
    let (bytes, report) = svg2ppt::convert_many(&refs, &opts)?;
    std::fs::write(&out, &bytes).with_context(|| format!("写入 {} 失败", out.display()))?;
    print_report(&out, &report, cli.quiet);
    if cli.strict && !report.is_empty() {
        eprintln!("--strict：存在降级，视为失败");
        std::process::exit(2);
    }
    Ok(())
}

fn print_report(out: &Path, report: &svg2ppt::Report, quiet: bool) {
    if quiet {
        return;
    }
    let simplified = report.simplified_points;
    if report.is_empty() {
        println!("已生成 {}", out.display());
    } else {
        println!(
            "已生成 {}（{} 处降级）",
            out.display(),
            report.diagnostics.len()
        );
        for (code, n) in report.summary() {
            println!("  - {code} ×{n}");
        }
    }
    if simplified > 0 {
        println!("  · 路径简化删掉 {simplified} 个顶点");
    }
}
