# NovelAI 常识（奶妹知识库）

> 给奶妹回答 NovelAI 相关问题用的事实卡。官方信息以 NovelAI 官网（novelai.net）与官方 Wiki 为准；
> 站内能做什么以「猫头鹰公益站·成员须知」为准——两者冲突时站内规定优先。

## NovelAI 是什么

- NovelAI 是一家 AI 创作订阅服务商，两大产品线：**文本生成**（storyteller，写故事/续写）和**图像生成**（NAI Diffusion 系列模型）。
- 图像生成在网页（novelai.net/image）和官方 App 里用，也提供**官方 API**（OpenAPI 风格，Bearer Token 鉴权，`/v1` 前缀）。猫头鹰公益站的网关兼容的就是这套官方 API 格式。

## 模型代际（图像）

| 代际 | 说明 |
|---|---|
| NAI Diffusion V3 | Anime V3，2023 年的世代，基于自研扩散模型 |
| NAI Diffusion V4 / V4（curated） | 2024 年底的新一代，支持多角色提示、更好的构图 |
| NAI Diffusion V4.5 / V4.5 (curated/full) | V4 的增强版，画质与提示跟随更好；猫头鹰站目前的主力之一 |
| NAI Diffusion V5 | 更新的世代；猫头鹰站对 V5 有单独的额度池与预设尺寸限制 |

- "curated" 与 "full" 的区别：训练数据审核程度不同，full 版可用范围更大。
- 站内免费规格：V4.5 及更早 ≤1024×1024（64 的倍数），V5 限三个预设尺寸，步数 ≤28（见成员须知）。

## 提示词（Prompt）写法

- **主体语法是 Danbooru 标签**：小写英文标签加下划线，如 `1girl, silver hair, red eyes, school uniform`。
- **逗号分隔标签**，从重要到次要排列；换行可视作分段，不影响权重。
- **权重**：`{标签}` 加强（约 ×1.05，可叠加），`[标签]` 减弱。V4/V4.5 还支持数字权重 `标签:1.2` 的写法（客户端端也接受）。
- **质量词**：正面常加 `best quality, amazing quality, very aesthetic, absurdres`；负面（undesired content）常加 `bad hands, extra digits, fewer digits, worst quality, low quality, jpeg artifacts, signature, watermark, username, blurry`。
- **角色/画风**：`artist:xxx` 指定画风；V4.5 起多角色用 `character:` 分块描述，配合 `|` 分隔（在官方界面里是「多角色提示」面板）。
- 常用结构：`画面数量与人物 → 角色特征 → 服装 → 动作/姿势 → 场景/背景 → 光影/构图 → 画风/质量词`。
- **undesired content（负面提示词）**默认模板就挺好；不要往负面里塞正面词。

## 常用生成参数

| 参数 | 说明 |
|---|---|
| steps | 采样步数；站内上限 28。20~28 之间收益最明显，再高收益小 |
| scale（CFG guidance） | 提示词跟随强度，常用 4~7；太高容易过饱和/崩坏 |
| sampler | 常用 `Euler ancestral`（Euler a）；k_euler_a 是同族写法 |
| seed | 固定 seed + 相同参数可以复现/微调一张图 |
| 尺寸 | 宽高必须是 **64 的倍数**（站内强制；官方也推荐竖图 832×1216、横图 1216×832、方图 1024×1024） |
| SMEA | 一种采样增强开关；站内免费规格自动关闭 |

## 图生图相关

- **img2img**：原图 + 强度（strength）与噪声（noise）决定改多少；strength 越大偏离原图越多。
- **Inpaint（局部重绘）**：框选区域重画，需要原图信息。
- **Vibe Transfer**：用参考图传"氛围/画风"，编码一张参考图约消耗 2 Anlas（站内 Vibe 走 Anlas 额度）。
- **Director / Enhance**：官方的局部修改与放大工具；站内由成员规则开关。

## Anlas 与订阅（官方侧常识）

- Anlas 是官方站的生成货币：出图超出订阅免费规格、Vibe、放大等会消耗。
- 订阅档位（Paper / Tablet / Scroll / Opus）影响免费生成规格与 Anlas 收入；Opus 档小尺寸出图基本不耗 Anlas。
- 这些是**官方站**的概念；猫头鹰公益站的"额度"是站长自设的站内规则，与官方 Anlas 无关。
