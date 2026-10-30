# nai-parity：官网 vs workbench 量化对照

用 ego-browser 打开官网生图页与本地/线上工作台，按同一套文本锚点采集控件几何与计算样式，
输出逐项差异表。官网数据始终是唯一基准。

## 跑法

```bash
# 1) 本地预览（不需要网关）：预置 boot 数据
cd web && pnpm dev --port 5174

# 2) 对照本地
TARGET=local ego-browser nodejs < tools/nai-parity/parity.mjs
# 对照线上
TARGET=prod  ego-browser nodejs < tools/nai-parity/parity.mjs
```

产物写到 `/Users/davidzhao/ego/owl-nai-1to1/`：`parity-official.json`、`parity-<target>.json`、
`parity-report.md`、`parity-<target>.png`。

## 判读规则

- 官网登录态挂着支付横幅（+52px），侧栏又是上下双向锚定，所以绝对 `y` 不可比。
  报告对滚动区内的控件用 `relY`（相对滚动容器顶部），对滚动区外的控件用绝对 `y`。
- `w` 差异经常只是文本节点宽度（行内 span 被 flex 撑满），以 `x`/`fs`/`fw`/`color`/`bg` 为准。
- 官网侧栏内容宽 373，外边距 12；滚动容器 400 宽（含 7px 滚动条）。

## 已知偏差（2026-10-30 采集）

| 项 | 官网 | 说明 |
| --- | --- | --- |
| 模型/模式行 | 36 高、1px `#22253F` 描边、radius 8、`padding: 4px 4px 4px 12px` | 已按此改为描边盒子 |
| 行标签 Model / Mode | 16px / 400 / 白 / 行高 20 | 已对齐（Position 行仍用 14px 次级样式） |
| Prompt / Undesired Content 标签 | 激活 `#22253F` 药丸；未激活 `opacity: .5`、白字 | 已改为 opacity 方案 |
| 组标签 Reference Images / Image Settings | 14px、白字 + `opacity: .5` | 已对齐 |
| Reference Images 组 | `Image2Image` / `Vibe Transfer` / `Precise Reference` 三张 373×67.3 卡 | 已按三卡重排 |
| 输入卡内部 | 图标 21、标题 16/600/24、副标题 0.72rem(11.52px)/70%、右侧两个 44×40 按钮 | 已对齐 |
| Resolution 行 | 高 28，右侧胶囊 117.5×28（`padding: 0 8px`）、宽高输入 39.8 | 数字已量到，样式待续 |
| Number of Images | 6 段，每段 57.2×30，激活底色 `#22253F` | 当前实现为 1–4 段，按网关额度另定 |
| 参数行 Steps/Guidance/Seed/Sampler | 标签起点 25 / 83 / 161 / 209 | 已用 58/78/48 列网格复现 |
| Anlas 徽章 | 左段 87×41（`radius 8 0 0 8`）+ 右段 42×41（`0 8 8 0`） | 已补上「+」段 |
