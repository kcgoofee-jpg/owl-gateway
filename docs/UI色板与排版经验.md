# UI 色板与排版经验

来源：[ZaynHao 的帖子](https://x.com/ZaynHao/status/2111685618925474098)（2026-10-18，设计工程师，帖子里的做法来自他做的 minttr 等产品）。
下面「帖子的做法」只写帖子里明确说的；「建议」是对照本仓库得出的，不是帖子的内容。

## 帖子的做法

### 1. 用 Radix Colors 的 12 级色板，按用途取色

每一级有固定用途，搭配时按用途选，组合就很少出错：

| 级 | 用途 | 级 | 用途 |
|---|---|---|---|
| 1 | 应用背景 | 7 | 控件边框、焦点环 |
| 2 | 微妙的背景 | 8 | 悬停时的控件边框 |
| 3 | 控件背景 | 9 | 纯色背景（强调色实心） |
| 4 | 悬停时的控件背景 | 10 | 悬停时的纯色背景 |
| 5 | 选中 / 活动的控件背景 | 11 | 低对比度文字 |
| 6 | 微妙的边框和分隔线 | 12 | 高对比度文字 |

同一个色相里，1–2 是背景，3–5 是交互组件，6–8 是边框和分隔线，9–10 是纯色，11–12 是文字。亮色和暗色各有一套官方色值。

### 2. 一个应用只选 3–5 种有个性的色

可以从自己喜欢的网站或应用里取色。他用的是 minttr 的背景色 `#f6f4f0`（偏暖的米白）。

### 3. 基本功

- 视觉对齐。
- 一页最多 3 种字号。
- 留意间距。
- 小技巧：0.5px 的边框。

### 4. 去掉 AI 味

- 到处加的「·」分隔符。
- 直接摆在页面上的解释文字。

## 建议（第 1 步换色板已做完，见 `web/src/ui/tokens.css`；剩下两步各自独立 PR，先看效果再决定）

2. **字号收到 3 档**：建议正文 14px、辅助 12px、标题 18px，其余合并。要跑排版检查（不许孤字、同排等高）。
3. **清理「·」和说明文字**：「·」改成间距或分栏；说明文字挪进悬停提示或「?」弹层，页面上只留数据。

### 注意

- **0.5px 边框**：非高清屏上会发虚甚至看不见，只在高清屏上用（`@media (min-resolution: 2dppx)`），其余保持 1px。
- **暗色**：Radix 的暗色是单独调过的一套，不是亮色反转，直接用官方的。
- **对比度**：换色后要重跑无障碍检查（axe），文字至少用 11 级（低对比）和 12 级（高对比），别用 9–10 级做正文。

## 待站长定

- 要不要做第 2、3 步。

## 后台管理页的框线规则（2026-10-31 定）

站长连着提了三次「这些线条太丑」，原因是我只改了看得见的那几处，没立规矩。现在定下来：

1. **装饰性的框线一律不要**：卡片外框、区块之间的横线、表格行线、分节标题上下的线、磁贴外框，
   全部去掉，分组只靠留白（`--sp-4` / `--sp-5` / `--sp-6`）和背景层次（`--surface` / `--surface-2`）。
2. **只有「可交互」的东西保留边**：`input` / `select` / `textarea` / `.btn` / `.switch` / `.dot`。
   这些边是「这里能点、这里能填」的提示，不是装饰。
3. **状态色只做语义，不做装饰**：风险卡片保留左侧 3px 语义条（红 / 黄 / 蓝），因为它是「这条属于哪种风险」的
   唯一信息；除此之外不再有彩色边框。
4. **假边线也要查**：`box-shadow: inset 0 -1px 0` 画出来的表和 `border-bottom` 一样刺眼（成员表头就是这么来的）。
   排查时用计算样式扫，不要只 grep `border`：

   ```js
   // 在后台任一页面的控制台里跑，列出所有还在画线的元素（跳过控件）
   for (const el of document.querySelectorAll('*')) {
     const cs = getComputedStyle(el), r = el.getBoundingClientRect();
     if (r.width < 60 || r.height < 6) continue;
     if (/^(BUTTON|INPUT|SELECT|TEXTAREA)$/.test(el.tagName)) continue;
     if (/\b(btn|input|chip|switch|seg|badge)\b/.test(String(el.className))) continue;
     const b = ['Top','Right','Bottom','Left'].filter(s => parseFloat(cs['border'+s+'Width']) > 0);
     const hair = cs.boxShadow !== 'none' && /inset 0 -?1px/.test(cs.boxShadow);
     if (b.length || hair) console.log(el.className || el.tagName, b.join(','), cs.boxShadow);
   }
   ```

5. **验收口径**：`/admin/members`、`/admin/upstream`、`/admin/logs`、`/admin/system`、`/admin/onboarding`、
   `/admin/limits` 和成员详情，上面那段脚本除了控件以外必须**零输出**。2026-10-31 实测全部为零。
