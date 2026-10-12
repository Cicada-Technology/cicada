# cicada · brand assets

标志把 **cicada** 的首字母 **C** 与一片蝉翼缝合成一体：外侧是温润的 C，内侧收成渐尖的翼形，镂空翅脉沿翼面展开。半透明的层次呼应 L0–L4 的分层架构，也呼应蝉破土而出、振翅起飞的意象。

打开 [`index.html`](index.html) 查看完整品牌规范页（也可在应用运行时访问 `/brand/index.html`）。

## 文件

| 文件 | 用途 | 建议背景 |
| --- | --- | --- |
| `logo.svg` | 横版标志，图形标 + 深色字标 | 浅色 / 白色 |
| `logo-dark.svg` | 横版标志，图形标 + 白色字标 | 深色 / 品牌蓝 |
| `logo-mark.svg` | 彩色图形标，适合头像、水印、独立使用 | 浅色 / 深色 |
| `logo-mark-mono.svg` | 白色单色标，翅脉为透明镂空 | 深色 / 品牌色 |
| `icon.svg` | 圆角方形 app icon / favicon | 任意 |
| `icon-square.svg` | 全出血方形源文件，PWA / Apple touch 导出用 | 任意 |
| `icon-192.png` / `icon-512.png` | PWA 图标（512 同时用于 maskable） | 任意 |
| `apple-touch-icon.png` | iOS 主屏图标 180×180 | 任意 |
| `wordmark.svg` | 纯字标，用于需要单独排版的场景 | 浅色 / 白色 |

站点图标位于 [`../favicon.svg`](../favicon.svg)（现代浏览器）与 [`../favicon.ico`](../favicon.ico)（16–64px 多尺寸 fallback）；PWA 清单位于 [`../site.webmanifest`](../site.webmanifest)。

## 色彩

| 名称 | 色值 | 用途 |
| --- | --- | --- |
| Signal Blue | `#0A84FF` | 主色、界面强调 |
| Sky | `#5FD3FF` | 标志渐变终点（翼尖） |
| Indigo | `#5E5CE6` | 标志渐变起点（翼根） |
| Ink | `#1D1D1F` | 浅色背景上的字标 |
| Mist | `#F5F5F7` | 深色背景上的字标 |
| Canvas | `#F2F2F7` | 页面底色 |
| Surface | `#FFFFFF` | 卡片 / 内容面 |
| Night | `#121214` | 深色画布 |

标志渐变只使用 Indigo → Signal Blue → Sky 三个锚点，不要改变方向或新增颜色。

## 使用

- 图形标四周至少留出 **图形标高度的 1/4** 作为清晰空间。
- 最小尺寸：图标 **16px**、彩色图形标 **24px**、横版标志宽度 **160px**。
- 不要拉伸、压缩、改色、加投影 / 描边 / 发光；复杂背景上请改用白色单色标或先铺纯色底。
- 图形标与字标的关系已经调好，不要重新拼版。

## 字体

字标为定制几何单线体：小写、圆头端点，x-height 44、stroke 8.75。界面文字沿用 `web/src/styles/tokens.css` 中的系统字体栈，不加载 Web 字体。
