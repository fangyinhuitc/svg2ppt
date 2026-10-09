# svg2ppt 设计文档

> 版本：v1（初稿） · 日期：2026-10-09 · 状态：已实现 M1
> 目标：把 SVG 转换成 `.pptx`，在「可编辑性」与「视觉保真度」之间提供明确可切换的三档策略。

---

## 1. 定位与目标

### 1.1 要解决的问题

SVG 是技术绘图、架构图、图标的事实标准；PPT 是汇报交付的事实标准。二者之间缺少一个**可靠的、可批量的、保真的**转换链路。现有常见做法有三类，都有明显缺陷：

| 做法 | 问题 |
| --- | --- |
| 截图 / 位图粘贴 | 放大糊、不可编辑、不可换色 |
| 手工在 PPT 里重画 | 成本高得离谱，且必然走形 |
| PowerPoint 直接「插入 SVG」 | 依赖客户端版本，脚本/批量场景不可用，且转换规则不可控 |

本项目要做的是一条**纯 Rust、无外部依赖、可脚本化**的 SVG → PPTX 管线。

### 1.2 目标

- **G1 三种输出模式**：矢量可编辑（`vector`）、位图保真（`raster`）、原生 SVG 嵌入（`native`），由用户按场景选择。
- **G2 默认矢量**：`vector` 模式输出的是真实的 DrawingML 形状（`<a:custGeom>` 路径），在 PowerPoint 里可选中、可改色、可缩放，而不是一张图。
- **G3 明确降级**：任何无法矢量表达的特性（滤镜、遮罩、图案填充、半透明…）都要**显式记录并给出降级动作**，不允许静默丢失。
- **G4 中文/字体正确**：通过 fontdb 加载系统字体，支持 `--font-dir` 指定字体目录，保证服务端与本机一致。
- **G5 库 + CLI 双形态**：核心是 `svg2ppt` crate，CLI 只是其一层薄封装。

### 1.3 非目标（v1）

- 不支持 SVG 动画、脚本、交互（usvg 只处理静态 SVG 子集，这是主动选择）。
- 不做 PPTX → SVG 反向转换。
- 不做滤镜的「矢量等价实现」（filter 一律走光栅降级）。
- 不做 GUI / WASM（后续可评估）。

---

## 2. 技术选型

选型原则：**优先选活跃维护的库；只有在库无法满足核心需求时才自己写代码，并且把自写部分隔离在单一模块内。**

### 2.1 SVG 解析 —— `usvg` 0.48.1

Linebender（原 RazrFalcon）维护，Apache-2.0 OR MIT，2026-10 仍在活跃提交。它是 resvg 的预处理层，是 Rust 生态里事实上的标准 SVG 解析库。

选择理由：

- 把 SVG 的复杂性**前置消化**：CSS 应用、`use` 展开、嵌套 `svg` 解析、`url(#id)` 引用解析、相对单位换算、marker 转普通元素、无效元素剔除，全部在解析期完成。
- 输出的是**极简强类型树**：所有形状已归一化为 `Path`（只含绝对 `MoveTo / LineTo / QuadTo / CurveTo / ClosePath`），弧形/相对指令已在解析期转换完毕。我们因此**完全不需要实现 SVG 路径语法解析**。
- 有约 1600 个 SVG→PNG 回归测试背书。
- 纯 Rust、无系统库依赖，跨平台输出可复现。

限制（接受）：只支持静态 SVG 子集，无动画/脚本；CSS 支持是最小集。

### 2.2 光栅化 —— `resvg` 0.48.1

同一项目，像素级跨平台一致，纯 Rust（tiny-skia + rustybuzz + ttf-parser）。用于 `raster` 模式整体渲染，以及 vector 模式中「子树降级」的局部渲染。

### 2.3 PPTX 写入 —— `office-toolkit` 1.0.0（feature = `powerpoint`）

这是整个选型中**唯一需要认真对比**的一环。候选与结论：

| 候选 | 版本/活跃度 | 关键能力 | 结论 |
| --- | --- | --- | --- |
| **`office-toolkit`** | 1.0.0（2026-07） | `Geometry::Custom(CustomGeometry)` + `PathCommand::{MoveTo, LineTo, CubicBezierTo, Close}`、嵌套 `ShapeGroup`（含 `chOff/chExt`、旋转、翻转）、`Picture`、`Fill::{Solid, Gradient, Pattern, Image}`、`TextBody` | ✅ **选中** |
| `rpptx` | 0.13.1（2026-10，极活跃） | 读写改、PDF 导出、模板 | ❌ 形状只有 **preset** 几何：`ShapeMut::set_auto_shape_type(preset)` 的文档明写「A custom geometry is replaced in the same schema slot」——它能读/替换 custom geometry，但**没有任何 API 写入任意 path**。SVG 的核心就是任意 path，故否决。 |
| `ppt-rs` | 0.2.27 | Markdown/HTML→PPTX、MCP | ❌ 定位是「文档生成」，不是矢量绘图后端；社区与维护不可与上者比。 |
| `ooxmlsdk` | 0.10.2 | schema 级强类型读写 | ❌ 是 SDK 而非高层 API，写一份 pptx 需手工组装大量 schema 类型，工作量接近自写。 |
| 自写 zip + XML | — | 完全可控 | ❌ 违背「尽量用现成库」原则，且需自行维护 OPC 关系/Content-Type/主题等易错细节。 |

`office-toolkit` 的关键能力已通过**下载源码实证**（`drawingml-1.0.0/src/model.rs`）：

```rust
pub enum Geometry { Preset(PresetShape), Custom(CustomGeometry) }

pub struct CustomGeometry { pub width_emu: i64, pub height_emu: i64, pub commands: Vec<PathCommand> }

pub enum PathCommand {
    MoveTo { x: i64, y: i64 },
    LineTo { x: i64, y: i64 },
    CubicBezierTo { x1: i64, y1: i64, x2: i64, y2: i64, x: i64, y: i64 },
    Close,
}

pub struct ShapeGroup { /* offset/extent/child_offset/child_extent/rotation/flip … */ pub shapes: Vec<Shape> }
pub struct Presentation { pub slides: Vec<Slide>, pub slide_width_emu: i64, pub slide_height_emu: i64, … }
```

**能力边界（这几点直接决定了降级策略，务必记住）**：

1. `PathCommand` **没有二次贝塞尔（`a:quadBezTo`）**——需要把 usvg 的 `QuadTo` 升阶为三次贝塞尔（数学上无损）。
2. `Color` **不带 alpha**（其文档明写 alpha modifier 不保留）——SVG 的 `opacity` / `fill-opacity` 无法精确表达，只能与背景色预乘近似。
3. `GradientFill` **只有线性渐变**（`stops` + `angle_60000ths`），没有径向/路径渐变。
4. `PictureFormat` 只有 `Png / Jpeg / Gif / Bmp`——`native` 模式（嵌入 `.svg` 媒体部件）需要绕过该库做包后处理，故排到 M2。
5. `Picture` 的位置由 `offset_emu/extent_emu` 决定，`shape_properties.transform` 不被采纳，**图片无法旋转**——带旋转/斜切的图片必须降级为光栅。
6. `Line.dash` 只支持 11 种预设虚线，自定义 `stroke-dasharray` 只能就近近似。

### 2.4 其余依赖

| 用途 | 库 | 说明 |
| --- | --- | --- |
| 光栅画布 | `tiny-skia` 0.12 | resvg 的渲染目标类型，需要直接依赖才能开 pixmap |
| CLI | `clap` 4（derive） | 事实标准 |
| 库错误 | `thiserror` | 库内错误类型 |
| CLI 错误 | `anyhow` | 顶层聚合 |
| 测试 | `zip` 9（dev-dependency） | 解开产物检查 OPC 包结构 |

> 关于 `image`：原计划用它把 WEBP 转码成 PNG。实现时改为**直接降级为位图**（反正 resvg 本来就会渲染它），少一个依赖、少一条代码路径。见第 6.5 节。

---

## 3. 三种输出模式

| 模式 | 原理 | 可编辑 | 视觉保真 | 客户端要求 | 状态 |
| --- | --- | --- | --- | --- | --- |
| `vector`（**默认**） | usvg 树 → DrawingML 形状树 | ✅ 完全可编辑 | 中～高（受降级矩阵影响） | 任意 | **M1** |
| `raster` | resvg 整页渲染 → PNG → Picture | ❌ | 最高（像素级） | 任意 | **M1** |
| `native` | 原 SVG 字节作为媒体部件嵌入 | 由 PowerPoint 决定 | 最高 | PowerPoint 2016+ | M2 |

选择建议：

- 汇报材料需要二次编辑/换色/抽取元素 → `vector`
- 复杂滤镜/渐变/遮罩的艺术图，或要 100% 还原 → `raster`
- 交付给使用新版 PowerPoint 的同事、希望他们在 PPT 里自行拆解 → `native`（M2）

---

## 4. 架构

### 4.1 管线

```
SVG 字节
   │
   ├─ [svg]     usvg 解析：CSS/use/引用/单位归一化 → usvg::Tree
   │
   ├─ [fit]     画布适配：slide 尺寸 × fit 策略 → 缩放 + 平移矩阵
   │
   ├─ [convert] 深度优先遍历 → 中间表示 IR（Vec<DrawItem>）
   │              ├─ 叶子节点用 abs_transform 逐点变换到画布坐标
   │              ├─ 不支持特性 → 局部光栅降级（resvg 渲染子树 bbox）
   │              └─ 每一步降级写入 Diagnostic
   │
   ├─ [emit]    IR → office-toolkit 模型（AutoShape / Picture / ShapeGroup）
   │
   └─           保存 .pptx
```

### 4.2 为什么要有 IR 层

IR（`src/ir.rs`）是与 OOXML 无关的纯几何/样式描述。存在的三个理由：

1. **隔离唯一的外部风险点**：`office-toolkit` 是 2026 年才发布 1.0 的新库，API 可能演进。有了 IR，替换后端只需重写 `emit.rs` 一个文件。
2. **可测试**：转换逻辑（几何/颜色/降级）可以在不生成 pptx 的情况下断言，单测更快更稳。
3. **可复用**：同一份 IR 未来可发射到 SVG / PDF / ODP 等其它后端。

### 4.3 模块划分

```
src/
  lib.rs        公开 API：convert() / ConvertOptions / ConvertReport
  svg.rs        usvg 封装：解析选项、系统字体加载、SVGZ 解压
  geom.rs       单位与几何：EMU 换算、fit 计算、矩阵分解、QuadTo→CubicBezier
  ir.rs         中间表示：DrawItem / Paint / StrokeStyle / TextItem / Diagnostic
  convert.rs    usvg::Tree → IR（核心映射规则，本项目最核心的文件）
  raster.rs     resvg 封装：整页渲染 + 子树局部渲染
  emit.rs       IR → office-toolkit 模型 → 保存
  cli.rs        clap 参数解析
  main.rs       入口
```

---

## 5. 坐标与单位

### 5.1 EMU

DrawingML 用 EMU（English Metric Units）作为唯一长度单位：

- 1 inch = 914 400 EMU
- CSS px = 1/96 inch → **1 px = 9 525 EMU**
- 点（pt）= 1/72 inch → 1 pt = 12 700 EMU
- 角度为 1/60000 度，顺时针为正
- 百分比类属性（如 alpha）为「千分之一百分比」：100% = 100 000

### 5.2 画布

| 预设 | 尺寸（EMU） | 英寸 |
| --- | --- | --- |
| `16:9`（默认） | 12 192 000 × 6 858 000 | 13.333 × 7.5 |
| `4:3` | 9 144 000 × 6 858 000 | 10 × 7.5 |
| `A4` / 自定义 `WxH` | 按输入计算 | — |

### 5.3 适配策略 `--fit`

`scale = f(svg_size, slide_size)`，外加居中偏移 `tx, ty`：

| 策略 | 行为 |
| --- | --- |
| `contain`（默认） | `min(sx, sy)`，等比居中，留白 |
| `cover` | `max(sx, sy)`，等比填满并居中裁剪（超出部分被画布裁掉） |
| `stretch` | `sx, sy` 分别缩放（会变形，一般不推荐） |
| `none` | 1:1（px→EMU），不缩放，可能溢出 |

### 5.4 变换的处理原则（重要）

usvg 的每个节点都带有从根累积下来的绝对变换矩阵（`abs_transform`）。处理原则：

- **Path**：**逐点变换**。把路径的每个控制点用绝对矩阵变换后，再转成 EMU 写进 `custGeom`。理由是：DrawingML 的 `a:xfrm` 不支持任意矩阵（只支持 offset/extent/rot/flip），而逐点变换可以精确表达任意仿射变换的结果。代价是旋转被「烘进」路径点，形状本身不再有 `rot` 属性——视觉等价，可编辑性略降。
- **描边宽度**：取变换矩阵的**平均缩放因子**（`sqrt(|det|)`）对 `stroke-width` 缩放，而不是分别用 sx/sy（描边在所有方向应等宽）。
- **Image**：图片无法逐点变换。若矩阵**不含旋转/斜切**（只含平移+缩放），直接用 `offset + extent`；否则**降级为光栅**（把变换后的图片区域渲染成 PNG）。
- **Text**：同 Path，逐点变换；字号按平均缩放因子换算。

---

## 6. SVG → DrawingML 映射规则（核心）

### 6.1 遍历策略

深度优先遍历 `usvg::Tree`，对三类叶子节点分别处理：`Path`、`Image`、`Text`。

- **不使用 PPT 的 group 承载 SVG 的 `<g>`**：而是把 group 的变换通过 `abs_transform` 累积到叶子节点上，做**扁平化输出**。
  - 优点：位置绝对精确，不受 `chOff/chExt` 坐标系换算误差影响，形状数量少、PPT 选择面板干净。
  - 代价：group 级别的 `opacity` 被近似为「累乘到子元素上」——对组内元素不重叠的情况完全等价，重叠时会有细微差异（可接受，记为诊断）。
- **不支持的特性触发「子树降级」**：遇到 `clip-path` / `mask` / `filter` / `pattern` 填充 / 径向渐变（默认）时，用 resvg 把**该节点 bbox 区域**渲染成 PNG，作为一个 `Picture` 插入到对应位置。这样局部保真，而不是让整页退化。

### 6.2 Path → AutoShape

| SVG | DrawingML |
| --- | --- |
| `usvg::Path.data` 段序列 | `Geometry::Custom(CustomGeometry { commands })` |
| `MoveTo` | `PathCommand::MoveTo` |
| `LineTo` | `PathCommand::LineTo` |
| `QuadTo(q, p)` | `PathCommand::CubicBezierTo`（升阶，`p0` 为当前点）<br>`C1 = p0 + 2/3·(q − p0)`<br>`C2 = p  + 2/3·(q − p )` |
| `CurveTo(c1, c2, p)` | `PathCommand::CubicBezierTo` |
| `ClosePath` | `PathCommand::Close` |
| 路径 bbox | shape 的 `offset + extent`；`custGeom` 的 `w/h` 取 bbox 尺寸，路径点相对 bbox 原点 |
| `fill-rule` | 记为诊断（库未建模，默认 nonzero）；evenodd 差异极小 |

> 注：usvg 已把 `rect` / `circle` / `ellipse` / `line` / `polygon` 全部转成 path，所以**不需要**分别处理基本形状。M2 可考虑增加「preset 识别」（把恰好是矩形的路径识别成 `prstGeom prst="rect"` 以便圆角可调），但正确性优先，M1 一律用 `custGeom`。

### 6.3 填充

| SVG | DrawingML | 说明 |
| --- | --- | --- |
| `fill="none"` | `Fill::None` | |
| `fill="#rgb"` | `Fill::Solid(Color::Rgb(hex))` | |
| `fill-opacity` / `opacity` | **预乘到背景色** | `Color` 无 alpha：`c' = c·α + bg·(1−α)`，默认 bg 为白，可用 `--background` 指定。记诊断 `alpha-approximated` |
| `linearGradient` | `Fill::Gradient(GradientFill { stops, angle })` | `angle = atan2(dy, dx)` 换算为 1/60000 度；stops 的 `offset` 映射为 `pos`，stop-opacity 同样预乘 |
| `radialGradient` | **降级**：整节点光栅 | `GradientFill` 无径向支持（M2 可考虑用中心方向近似为线性） |
| `pattern` | **降级**：整节点光栅 | |

### 6.4 描边

| SVG | DrawingML |
| --- | --- |
| `stroke="none"` | 不写 `<a:ln>` |
| `stroke` 颜色 | `Line.fill = Fill::Solid(...)`（同上预乘 alpha） |
| `stroke-width` | `Line.width_emu`（按平均缩放因子缩放，最小 1 EMU） |
| `stroke-linecap: butt/round/square` | `Line.cap = Flat / Round / Square` |
| `stroke-linejoin: miter/round/bevel` | `Line.join = Miter / Round / Bevel` |
| `stroke-dasharray` | 就近匹配 11 种 `PresetLineDash`；无法匹配则记诊断并放弃虚线 |
| `marker-*` | usvg 已在解析期转为普通 path，无需处理 |

### 6.5 图片

- `usvg::Image.kind` 为内嵌编码数据。`PictureFormat` 只接受 `Png/Jpeg/Gif/Bmp`：
  - 直通：PNG、JPEG、GIF（字节原样嵌入，不重编码，零损失）
  - 其余（WEBP、嵌套 SVG）→ **降级为光栅**（诊断码 `unsupported-image-rasterized`）
- 位置/尺寸由 usvg 解析后的 bbox（已含 `preserveAspectRatio` 语义）确定
- 若带旋转/斜切 → **降级为光栅**（`Picture` 的位置只由 offset/extent 决定，表达不了旋转）

### 6.6 文本

两种策略，由 `--text-mode` 选择：

| 模式 | 做法 | 可编辑 | 保真 | 状态 |
| --- | --- | --- | --- | --- |
| `auto`（**默认**） | 能安全表达的单行文本 → `AutoShape(is_text_box=true)` + `TextRun`；其余回落转曲 | 部分 ✅ | 中～像素级 | **已实现** |
| `path` | 取 `Text::flattened()`（usvg 已排版好的字形轮廓子树）当作普通 Path 输出 | ❌ | ✅ 像素级 | 已实现 |

选「混合」作为默认的理由：纯 `path` 保真但对方**不能改字**（连错别字都改不了），纯 `editable` 又会让 PPT 的排版引擎接管行距/换行/字距从而偏移。折中方案是只在「排版结果不会漂移」时给文本框。

`auto` 走文本框的条件（任何一条不满足就转曲，并给出对应诊断码）：

| 条件 | 不满足时的诊断码 |
| --- | --- |
| 只有一个 text chunk（单行） | `text-multiline-fallback` |
| 线性排版（非 textPath） | `text-on-path-fallback` |
| 水平书写 | `text-vertical-fallback` |
| 无旋转/斜切（含逐字形 `rotate`） | `text-rotated-fallback` |
| 无逐字形 `dx/dy` | `text-per-glyph-offset-fallback` |
| 无 `letter-spacing` / `word-spacing` | `text-spacing-fallback` |
| 无 `textLength` / `lengthAdjust` | `text-length-adjust-fallback` |
| 无 `baseline-shift` / 上下标 | `text-baseline-shift-fallback` |
| 无 `font-variant: small-caps` | `text-small-caps-fallback` |
| 填充为纯色（非渐变/图案） | `text-paint-fallback` |
| 无描边、且可见 | `text-stroked-fallback` / `text-hidden-fallback` |

几何与字号换算：

- **文本框的框**用 `Text::bounding_box()`（SVG 的「排版盒」，上边界 = 基线 − 字体 ascent，高 = ascent + descent），而不是字形紧包围盒——它恰好对应 PPT 文本框的**行盒**语义，基线因此能自动对齐。
- 字号：`scale`（`avg_scale(total)`）已经是「1 SVG px = 多少 EMU」，所以 `sz = font_px × scale ÷ 12700 × 100`。640 px 宽的 viewBox 铺满 16:9 画布时，28 px → `sz="4200"`（42 pt）。
- `bodyPr` 四个内边距显式置 0（否则 PowerPoint 会塞进默认 0.1 英寸留白），`wrap="none"`、`anchor="t"`、`noAutofit`；`cNvSpPr txBox="1"` 标记成真文本框；形状壳自身 `noFill` 无边框。
- `text-anchor` → 段落 `algn`（`start→l` / `middle→ctr` / `end→r`）；一个 chunk 内的多个 `<tspan>` → 同一段落的多个 run（各自的颜色/粗斜体/下划线/删除线）。
- 字体族取字体栈首项；CSS 通用族映射成 PowerPoint 存在且默认有的字体（`sans-serif→Arial`、`serif→Times New Roman`、`monospace→Consolas`、`cursive→Comic Sans MS`、`fantasy→Impact`）。

实现要点（踩过的坑）：`Text::flattened()` 返回的是一个**子根** Group，其 `abs_transform` 为**单位矩阵**，而字形 Path 的 `abs_transform` 已经是「相对该子根、且已归一到 SVG 用户坐标」的值（实测：`basic.svg` 中 640×360 画布上的文本字形 bbox 直接就是 `(42.9, 306.9, 305.5, 29.0)`，与源坐标一致）。所以**不能**再额外叠加 `text.abs_transform()`，否则坐标会被重复放大。代码里用 `flat.abs_transform().is_identity()` 做了运行期判定以兼容该行为。

**已知残差（可编辑模式的固有代价）**：PPT 放置首行基线用的是它自己的字体度量（Office 系多用 OS/2 typo 表，usvg 的 bbox 用 hhea 表），两者在 PingFang SC 这类中日韩字体上可差 5%~6% em。实测 42 pt 的中文行基线整体偏上约 2.6 pt（约 0.9 mm），横向几乎无偏差（<0.5 px）。这个偏差随渲染器而变（PowerPoint / LibreOffice / Keynote 各不相同），无法在转换期消除——追求像素级对齐时用 `--text-mode path`。

### 6.7 背景

- `--background <color>`：在 slide 上放一个铺满画布的矩形作为底层（DrawingML 的 slide 背景需要 `p:bg`，库未建模，用底层矩形等价实现）。
- 默认透明（不画）。

---

## 7. 保真度矩阵与降级策略

| SVG 特性 | vector 模式 | 降级动作 | 诊断码 |
| --- | --- | --- | --- |
| 基本形状 / path | ✅ 完整 | — | — |
| 实色填充 / 描边 | ✅ 完整 | — | — |
| 线性渐变 | ✅ 完整 | — | — |
| 描边端点/连接/虚线（预设） | ✅ 基本 | — | — |
| 变换（平移/缩放/旋转/斜切） | ✅ 逐点变换 | — | — |
| 分组 `<g>` | ✅ 扁平化 | group opacity 累乘近似 | `group-opacity-flattened` |
| 半透明（opacity / fill-opacity） | ⚠️ 近似 | 与背景色预乘 | `alpha-approximated` |
| 径向渐变 | ❌ | 子树光栅 | `radial-gradient-rasterized` |
| 图案填充 | ❌ | 子树光栅 | `pattern-rasterized` |
| 滤镜（feGaussianBlur 等） | ❌ | 子树光栅 | `filter-rasterized` |
| clip-path | ❌ | 子树光栅 | `clip-path-rasterized` |
| mask | ❌ | 子树光栅 | `mask-rasterized` |
| 自定义 dasharray | ⚠️ 近似 | 就近预设 | `dash-approximated` |
| 带旋转的图片 | ❌ | 子树光栅 | `rotated-image-rasterized` |
| 单行纯色文本（`auto` 模式） | ✅ 可编辑 | 写成 `txBody` 文本框 | — |
| 其它文本 | ⚠️ 转曲 | 字形轮廓 | `text-as-path`（`path` 模式）或上表各 `text-*-fallback` |
| 动画/脚本/交互 | ❌ | 忽略（usvg 不解析） | — |

**降级不是失败**：每一条降级都会进入 `ConvertReport`，CLI 默认打印摘要，`--strict` 则让任何降级变成错误退出（适合 CI 守门）。

---

## 8. 公开 API

```rust
pub struct ConvertOptions {
    pub mode: Mode,               // Vector | Raster | Native(M2)
    pub slide: SlideSize,         // 16:9 | 4:3 | Custom(EMU, EMU)
    pub fit: Fit,                 // Contain | Cover | Stretch | None
    pub background: Option<Rgb>,  // 用于 alpha 预乘与背景填充
    pub text_mode: TextMode,      // Auto（默认，可编辑优先）| Path（全部转曲）
    pub scale: f32,               // raster 模式额外倍率（默认 2.0 ≈ 192dpi）
    pub font_dirs: Vec<PathBuf>,
    pub strict: bool,
}

pub struct ConvertReport { pub diagnostics: Vec<Diagnostic> }

// 单文件：SVG 字节 → pptx 字节
pub fn convert(svg: &[u8], opts: &ConvertOptions) -> Result<Vec<u8>>;

// 多文件：每个输入一页，合并为一个 pptx
pub fn convert_many(svgs: &[&[u8]], opts: &ConvertOptions) -> Result<Vec<u8>>;
```

---

## 9. CLI

```
svg2ppt [OPTIONS] <INPUT>...

参数:
  <INPUT>...                 输入 SVG 文件（支持多个，默认合并为一个 pptx 的连续页）

选项:
  -o, --output <FILE>        输出路径（默认 <input>.pptx）
      --mode <MODE>          vector | raster            [默认: vector]
      --size <SIZE>          16:9 | 4:3 | WxH(px)       [默认: 16:9]
      --fit <FIT>            contain | cover | stretch | none  [默认: contain]
      --background <COLOR>   背景色，如 #ffffff / white
      --text-mode <MODE>     auto | path               [默认: auto]
      --scale <F>            raster 模式缩放倍率        [默认: 2.0]
      --simplify <PX>        折线简化容差（SVG px），0 = 关闭  [默认: 0.2]
      --font-dir <DIR>       额外字体目录（可重复）
      --per-file             每个输入单独输出一个 pptx
      --strict               任何降级即报错退出
  -q, --quiet                不打印降级摘要
  -h, --help
```

退出码：`0` 成功；`1` 转换失败；`2` `--strict` 且存在降级。

---

## 10. 测试策略

| 层级 | 内容 |
| --- | --- |
| 单元 | EMU 换算、fit 计算、Quad→Cubic 升阶、矩阵分解、alpha 预乘、颜色解析、折线简化（共线删除 / 拐角保留 / 容差 0 / 退化点） |
| 金样 | `tests/golden.rs` 比对 `slideN.xml` 的逐字节快照，锁住 emit 层（详见 13.6，含两条硬约束） |
| 集成 | `tests/fixtures/*.svg` → 生成 pptx，解压断言：页数、`<a:custGeom>` 数量、media 部件数与 content-type 正确、诊断码、简化行为 |
| 降级断言 | 给每个 fixture 断言其诊断码集合（例如含 filter 的 fixture 必须产出 `filter-rasterized`） |
| raster 回归 | `raster` 模式产出的 PNG 与 resvg 直接渲染结果做像素比对，保证「不走样」 |
| 真实打开校验 | 生成后用 Python `python-pptx` / LibreOffice 无头转换验证不触发修复提示（CI 可选） |

fixture 至少覆盖：中文文本、线性渐变、旋转变换、内嵌图片、滤镜（触发降级）、无 viewBox、SVGZ。

现有 fixture：`basic.svg`（渐变/半透明/旋转/中英混排单行文本）、`filtered.svg`（滤镜 + 径向渐变）、`text.svg`（单行多 run 应可编辑；旋转/多行/字距三条应带码回落）、`image.svg`（内嵌 PNG 落位）。

---

## 11. 里程碑

| 阶段 | 内容 |
| --- | --- |
| **M1（已完成）** | CLI + `vector` 模式（path / 实色 / 线性渐变 / 描边 / 图片 / 文本转曲 / 变换）+ `raster` 模式 + 子树降级 + 诊断报告 + 集成测试 |
| **M2-A（已完成）** | `--text-mode` 混合模式（可编辑文本框 + 带码回落）、修掉图片 bbox 的重复变换 |
| **M3-A（已完成）** | 三平台 CI（ubuntu / macOS / Windows）、金样测试、折线简化（体积优化） |
| M2-B | `native` 模式（包后处理注入 SVG 媒体部件）、多行可编辑文本（自算行距）、preset 形状识别、slide 背景/主题、配置文件支持 |
| M3-B | 库 API 稳定化、批量目录转换（递归目录）、金样覆盖 raster 模式 |

---

## 12. 已知风险与对策

| 风险 | 影响 | 对策 |
| --- | --- | --- |
| `office-toolkit` 是 2026 年新库，API 可能演进 | 中 | IR 隔离，耦合点仅在 `emit.rs`；锁版本 |
| `Color` 不支持 alpha | 中 | 背景色预乘；`--strict` 可暴露 |
| 可编辑文本的基线与 SVG 差 2~3 pt（渲染器字体度量差异） | 中 | 已由 `auto` 模式限定在单行简单文本；像素级要求时用 `--text-mode path`（详见 6.6） |
| 服务端无中文字体 | 高 | fontdb 加载系统字体 + `--font-dir`；无字体时报错而非静默渲染豆腐块 |
| 大量 path 导致 pptx 体积/形状数膨胀 | 低 | **已缓解**：折线简化默认开启（`--simplify 0.2`，实测 -69% 体积，见 13.6）。曲线段与形状数本身未优化 |
| custGeom 坐标精度（EMU 为整数） | 低 | 逐点四舍五入；缩放后误差 < 1/9525 px，不可见 |

---

## 13. M1 实现现状与验证

### 13.1 交付物

```
docs/design.md            本文档
src/geom.rs               单位换算 / 画布适配 / 仿射矩阵 / 贝塞尔升阶（含 7 个单测）
src/ir.rs                 中间表示 + 诊断报告
src/svg.rs                usvg 封装（系统字体 + 自定义字体目录）
src/raster.rs             resvg 封装（整页渲染 / 单节点渲染）
src/convert.rs            核心：usvg 树 → IR（映射规则、降级判定）
src/emit.rs               IR → office-toolkit → pptx 字节
src/lib.rs                公开 API：convert() / convert_many()
src/main.rs               CLI（clap）
examples/inspect.rs       调试：打印 usvg 各节点的绝对变换
examples/ir.rs            调试：打印转换后的 IR（不写 pptx）
tests/fixtures/*.svg      测试素材
tests/integration.rs      端到端测试（解包检查 custGeom / media / 诊断码）
```

### 13.2 实现与设计的差异

| 设计 | 实现 | 原因 |
| --- | --- | --- |
| WEBP 用 `image` crate 转码 | 直接降级为位图 | resvg 本来就会渲染它，转码是多余的一层；少一个依赖 |
| 全局「子树降级」递归 | 同样递归，但 `Text` 的字形子树不参与降级判定 | 字形已是纯 Path，不会携带 filter/clip |
| `--radial-as-linear` 开关 | 未实现（M2） | 径向→线性近似的视觉效果不稳定，不如老实光栅化 |
| `Native` 模式 | 未实现（M2-B） | 该库不支持 SVG 媒体部件，需要做 OPC 包后处理 |
| 多行文本走文本框 | 未实现（M2-B） | 需要自算「行位置 → 段落 spcBef/lnSpc」，跨渲染器不稳定，先回落 |
| `TextMode::Editable`（无条件可编辑） | 取消该变体 | 旋转/多行文本必然偏移，与其给一个「看起来能用其实会漂」的开关，不如只留 `auto` 与 `path` |

### 13.3 验证结果

- `cargo test`：**25 项全通过**（10 单元 + 2 金样 + 12 集成 + 1 doctest），`cargo clippy --all-targets` 零警告。
- 视觉验证：LibreOffice 无头转 PDF → 渲染 PNG，逐页比对。
  - `basic.svg`（渐变圆角矩形 + 半透明圆 + 三角形 + 旋转 20° 的矩形 + 中英混排文本）**逐元素还原**，位置、角度、渐变方向、混色均正确。
  - `filtered.svg`：滤镜矩形与径向渐变圆按预期降级为位图并保真贴回原位，未降级的绿色矩形保持矢量。
  - `text.svg`：居中的双色单行文本成为**一个文本框两个 run**（`<a:t>红色</a:t><a:t>与蓝色</a:t>`，`algn="ctr"`）；旋转 / 多行 / 字距三条按预期带码回落为轮廓，视觉无损。
  - `image.svg`：内嵌 PNG 落位与源 SVG 完全一致（`off=(1219200, 2209800)`、`ext=(2438400, 2438400)`）。
- 文本几何量化比对（`basic.svg`，42 pt 中文行）：横向偏差 < 0.5 px，纵向基线偏上约 2.6 pt —— 属渲染器字体度量差异，见 6.6 节。
- 诊断链路：`--strict` 在有降级时以退出码 2 结束，可直接用于 CI 守门。

### 13.4 实现期踩到并已修掉的坑

1. **描边宽度单位二次换算**：`avg_scale(total)` 返回的已经是「1 SVG px = 多少 EMU」，代码里又套了一层 `px_to_emu`，导致形状框被撑大 9 525 倍（表现为形状尺寸达到 5 亿 EMU）。修掉后 `basic.svg` 的矩形框回到 `200×120 px` 对应的 `3 810 000×2 286 000 EMU`。
2. **文本展平子树的变换语义**：见 6.6 节——不能盲目叠加 `text.abs_transform()`。
3. **图片 bbox 的重复变换**：`Image::abs_bounding_box()` 只叠加了**父级**变换，不含图片自身的 `image_ts`（usvg `parser/image.rs` 里写的是 `view_box.transform(parent.abs_transform)`），而调用方又乘了包含 `image_ts` 的 `total`，等于把 `x/y` 平移量算了两次。改用局部 `bounding_box()` 后落位精确（`image.svg` 的 `off` 与换算值逐位相等）。这个 bug 在 M1 没被发现，是因为当时没有带图片的 fixture。
4. **`abs_layer_bounding_box()` 返回 `Option`**：空节点必须走 `else` 分支而不是 unwrap。
5. **借用冲突**：`self.page.items.push(... { name: self.next_name() })` 会同时可变借用 `self` 两次，需要先把 name 取到局部变量。

### 13.5 尚未覆盖

- 多行文本仍走转曲（`text-multiline-fallback`）：要可编辑就得自算行距并接受 PPT 的换行重排，留到 M2-B。
- `native` 模式（嵌入原 SVG 媒体部件）未实现。
- 库 API 未做稳定化承诺；不支持递归目录批量转换。
- 未见真实企业模板（母版/主题）适配。

### 13.6 M3-A：工程化（CI / 金样 / 体积）

**三平台 CI**（`.github/workflows/ci.yml`）：`ubuntu-latest` / `macos-latest` / `windows-latest` 矩阵，跑 build + test + `clippy -D warnings` + `fmt --check`，`fail-fast: false`（跨平台差异正是要发现的东西）。Linux runner 缺中文字体，显式装 `fonts-noto-cjk`，否则含中文的 fixture 会挂。

**金样测试**（`tests/golden.rs`）。两条硬约束：

1. 比对的必须是 `ppt/slides/slideN.xml`，**不能是整个 pptx**。`office-toolkit` 写 zip 时用当前时间填 DOS 时间戳，同一输入两次运行字节不同（实测确认；`slide_xml_is_deterministic` 守着这个前提）。XML 内容本身完全确定。
2. 金样 fixture **不能含文本**。字形轮廓来自系统字体，三平台必然不同。所以新增 `golden-geom.svg`（纯几何：圆角矩形+描边、椭圆+线性渐变、三次/二次贝塞尔、带旋转缩放的半透明分组、四段式虚线），文本回归交给 `integration.rs` 的行为断言。

**折线简化**（`geom.rs::simplify_mask` + `convert.rs::simplify_cmds`）：道格拉斯-普克，显式栈实现（数万点的递归会爆栈）。三条设计决策：

- **只简化 `MoveTo` 之后的连续 `LineTo`**。贝塞尔的控制点不是路径上的点，删任何一个都会改变曲线形状，一律不动。这是安全的子集，也是与「通用路径简化」的关键区别。
- 容差单位是 **SVG 用户坐标的 px**（`--simplify`，默认 0.2），在 `build_shape` 里乘 `avg_scale(total)` 换算成 EMU 再比较。默认 0.2 px 在铺满 16:9 时约 0.1 mm。
- bbox 在简化**之前**由 `track` 累积完整，因此简化不会让形状框变小（宁留余量），也就不影响 `a:off`/`a:ext`。

实测（2000 点正弦折线，640×360）：顶点 1999 → 122（删 1877 个），pptx 24 881 → 7 812 字节（-69%）。LibreOffice 渲染后逐像素比对，1889 个差异像素全部落在线条抗锯齿边缘（占画面 0.15%），无形状变形。

`Report` 新增 `simplified_points` 计数。**注意它不是诊断**，不进 `--strict` 判定——简化不是信息丢失。踩到的坑：`Report::extend` 最初只合并了 `diagnostics`，导致 `convert_many` 时统计只剩最后一页，已修为累加。
