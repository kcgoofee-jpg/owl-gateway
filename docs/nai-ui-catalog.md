# NovelAI 生图页控件档案（owl 复刻依据）

> 采集方式：ego lite 登录态下用 DOM/计算样式/React fiber 内省逐控件测量（2026-10），
> 只记录几何、颜色、层级与行为语义；实现代码全部为 owl 仓库原创。
> 视口基准 1438×756（dpr2）。色板：底 #13152C / 面板 #191B31 / 内嵌 #0E0F21 /
> 描边与按钮底 #22253F / 强调 #F5F3C2 / 强调底字 #13152C。字体 Source Sans Pro（owl 用 Source Sans 3 代替），
> 标题衬线字体 Eczar 700。按钮悬停统一 `background 0.08s ease-in-out`。

## 1. 侧栏导航行（nav-row，y52 高 62，与画布顶工具同排）

| 控件 | 几何 | 样式/组件 | 行为与后端 | owl 对应 |
| --- | --- | --- | --- | --- |
| 取消选择（笔尖 logo 图标） | 44×40，radius 4，透明底 | aria `deselect image`，icon 16×18 | 点击取消画布选中，隐藏图片工具条 | 同名按钮，OwlIcon |
| Anlas 徽章 | 左段 87×41 + 计数 | 「Anlas: N ✦」600 14px/21px，底 #22253F 左圆角 8 0 0 8 | 展示余额（/v1/me → today.anlas） | 同款，读 meSignal |
| Anlas 充值 + | 42×41，radius 0 8 8 0，pad 9 14 | icon 14×14 | 打开购买 Anlas 弹层 | 未开放 → toast |
| 汉堡菜单 | 44×40，radius 4，底 #22253F | aria `menu`，hover 展开 | 见下 | 同款 hover 面板 |

汉堡菜单（345 宽，x20 起，radius 5 条目 44 高）：分组头 600 14px/21px 41 高；
条目 bg #191B31 icon20+label，600 16px/24px。
Account：Account Settings / Logout。Other：Explore / Quick Start Gallery / Director Tools /
Opus Usage Limit / Inventory / Text Generation / Tokenizer。Help：Tutorial / Documentation / NovelAI Discord。
尾行版本号。owl：Account Settings→旧版工作台、Logout→登出、Quick Start→画廊锚点、
Director Tools/Inventory/Tokenizer 等未开放 → toast。

## 2. 侧栏滚动区（宽 385+7 滚动条，pad-left 12）

- **Model/Mode 行**（高 36）：`Model` 标签 14px + 下拉控件 107×28（底 #22253F radius 4，
  值 600 14px/18px + 箭头）。菜单宽 300 max-h 300，条目=标题 700 14px/21px + 描述 400 14px/21px，
  选中底 #13152C 字 #F5F3C2；分组头 New/Legacy 500 12px/18px 60% 白。
  `Mode` 钮 78×28（✻+Anime），点击循环模式并 toast「Model mode switched to …」。
- **Prompt 卡**（373×241）：头 #0E0F21 radius 8 8 0 0 pad 12 12 0——Tab 药丸
  （Prompt / Undesired Content，外壳 pad 2 5 radius 4，激活底 #22253F 字 600 14px）+
  右侧两个 30×30 图标钮（全屏编辑器 / 交换提示词）。正文 textarea（#0E0F21，pad 5 12）；
  尾部 #0E0F21 radius 0 0 8 8：Transparent BG 勾选药丸（✓/✗ 图标）+ Quality Preset 下拉
  （菜单 180 宽，条目 40 高，Standard/Light/None，选中 #13152C/#F5F3C2），
  下方 token 计量条（高 14，内条 4px，白色填充）。
- **Character Prompts 卡**（373×176，bg #191B31 radius 8 pad 12）：标题 600 16px/24px +
  描述 400 14px/21px + 右侧 + 钮 44×40 底 #22253F；Position 行 = 标签 +
  分段壳（radius 8 pad 4 边框）内 AI's Choice 138×30 / Custom 138×30（激活 #22253F）+ 地图小钮 33×30；
  「Clear Random Prompt Characters」钮 249×28 radius 3 pad 2 10。
  角色条目头（拖柄+角色图标+名字输入+右端 4 个 34×30 图标钮，末位激活底 rgb(45,47,72)）。
  owl：+ 与 Position 均未开放 → toast；卡面结构照搬。
- **Reference Images / Image2Image 卡**（373×93）：组标签 60% 白 14px；卡内图标 +
  「Image2Image」600 16px +「Transform your image.」400 14px + 右端上传/编辑两钮 44×40。
  owl：两钮 → toast（img2img 未开放）。
- **Image Settings**（373×184）：组标签；`Resolution` 700 16px + 右侧胶囊
  [宽输入 40×20][交换 12×16][高输入 40×20]（底 #22253F radius 4）；下一行
  分类下拉 Normal 112×34 + 方向分段三钮 78×30（横/竖/方，激活 #22253F）；
  `Number of Images` 700 16px + 四段 88×30 全宽（1-4，激活 #22253F，分隔线 1px）。

## 3. 侧栏 footer（y554 高 203，pad 12，gap 10）

- **参数行卡**（375×63，bg #191B31 radius 8 边框）：Steps / Guidance / Seed / Sampler 四组，
  标签 400 14px/21px 70% 白，值钮 600 16px/24px；Seed 是图标钮（用当前图种子），
  Sampler 值钮开下拉（菜单 170 宽，条目 156×40，分组 Recommended / Other，
  推荐=Euler Ancestral，其余 Euler、DPM++ 2S Ancestral、DPM++ 2M SDE、DPM++ 2M、DPM++ SDE）。
  右端 ▶ 钮 42×61（pad 0 12 0 0，icon 16）。
- **额度条**（375×56，bg #0E0F21 radius 8 pad 5 8 7）：`N% of Opus Generations remaining`
  600 13px/19.5px + More Info（#F5F3C2 600 12px/18px）；进度槽 333×6 内填 6px；
  右下 + 钮 18×18 底 #22253F radius 3。owl：换成每日免费额度文案，More Info→toast。
- **Generate**（375×40，bg #F5F3C2 字 #13152C 700 16px/24px radius 4 pad 4 8 4 12 gap 8）：
  文案 `Generate N Image(s)` + 徽章（bg #13152C 字 #F5F3C2 600 16px radius 4 pad 0 8 gap 4，
  N + Anlas 图标 10×10）；过渡 `color, background-color, transform .1s/.1s/.15s`；
  忙碌时变灰禁用。owl：文案与徽章一致，N=张数，徽章=本次计费（0 或 —）。

## 4. 画布

- **空画廊**：hero 标题 `Get Started` Eczar 700 20px/30px #F5F3C2（含原创星形装饰 SVG
  691×258 槽位），副题 400 16px，提示 400 16px（margin-top 35px）；
  画廊 3 列 masonry：列宽 ~259px、列距 4、左右留白 ~50px、卡 radius 8、按图自然纵横比，
  点击复制提示词。装饰层同槽原创绘制。
- **生成中**：画布顶部进度条（槽 866×4 圆角，黄 #F5F3C2 填充往返滑动）；
  右轨新增 spinner 卡（107×107 底 #191B31 选中黄描边，白弧旋转）；
  画布占位 tile（底色深，中心白点）；Generate 禁用变灰；底部种子胶囊立刻写入本次种子。
- **选中态顶部工具条**（居中一组，条目 radius 3 透明底，42×42 图标钮 / 96×42 带徽章钮）：
  enhance ✨ 42 | variations 96（数字+Anlas 角标）| upscale 96（数字+Anlas 角标）|
  set-init 42 | edit 42 | inpaint 42 | send-to-tools 42。
  后端：enhance/variations/upscale 走 augment 类接口（当前账号额度不足时官方弹
  「The paint's run dry.」全高付费弹层，bg #191B31 radius 3 pad 55 20 20，× 右上）。
  owl：这四个+edit/inpaint/send-to-tools 均 toast「未开放」；按钮与布局照常在。
- **选中态底部条**：左组=尺寸胶囊（同上 W/swap/H）+ 齿轮 42×42（display.settings，
  画布显示设置）+ 种子胶囊 139×32（底 #22253F radius 3 pad 12 gap 11，🌱+种子号，
  点击 Copy to Seed）；右组=pin / copy / download 三钮 42×42。
  owl：齿轮→toast；种子胶囊→写入种子框；copy→复制提示词；download→下载；pin→本地收藏。
- **画布 tile**：377×551（832×1216 等比），radius 8，底 #13152C，
  过渡 `transform .25s cubic-bezier(.22,1,.36,1)`；未选中 tile 悬停出现
  种子小徽章（40×42 底 #22253F）等快捷钮。
- **右轨 History**（140 宽，bg #191B31）：头钮 `History ▸` 84×40（600 16px/20px pad 7 12）；
  列表条目 107×107 gap 10 左右 pad 10，选中黄描边；底排 lock/download-all/delete 38×28
  （600 14px/18px pad 4 12）。owl：lock 切换滚动锁定；download-all 用 fflate 打包；
  delete 清空历史（先确认）。

## 5. 全局

- Toast：Toastify 右上角（~350 宽，bg #22253F radius 2，400 16px/24px 白），
  错误同样式；owl 在 nai 模式把 `.toasts` 覆盖为右上同款。
- 顶栏 52px 是公告条（bg #22253F，黄字 700 16px + × 40×32）；无公告时不存在。
- 模型清单：New=V5 Curated/V5 Full；Legacy=V4.5 Curated/V4.5 Full/V4 Curated/V4 Full/
  Anime V3/Furry V3 —— 与 owl BASE_MODELS+V5_MODELS 一一对应。
- Quality：Standard/Light/None；UC Preset：None/Light/Heavy/Human Focus/Furry（owl 同）。

## 6. 移动端（≤860px，官方 390×844 实测 2026-10-29）

官方手机版是**单列布局**，桌面三栏全部重排，无并排 400px 面板：

- **导航行**（48px，bg #191B31）：取消选择 40×28 · Anlas 药丸（仅数字+⚡，无
  "Anlas:" 前缀）+ 充值 · 汉堡 40×28。
- **额度条**（导航行下，bg #191B31）："N% of Opus Generations remaining + More Info"
  + 进度条 + ➕；桌面版这根条在页脚里。owl 同位复用 `nai-quota`（文案为站点额度口径）。
- **画布**：占满其余空间；空状态 hero（Eczar "Get Started" /
  "Get Inspiration from our quick start gallery!" / "Click an image to copy the prompt."）
  + 两列瀑布流（列距 4、左右 10）。
- **折叠提示词条**（44px，停靠页脚上方）：单行提示词预览 + ▲ 展开箭头；点开是
  **底部抽屉**（官方为可滚动面板：Prompt/Reference Images/Chunks 标签、Prompt/UC、
  编辑区、Transparent BG…）。owl 把侧栏滚动区（模型行/提示词卡/图生图/设置卡）整体
  滑入为抽屉，遮罩点击收起。
- **页脚**（40px 高，左右 10 间隙 8）：齿轮 48×40（#22253F）｜Generate 1 Image
  258×40（#F5F3C2 + Anlas 徽标）｜时钟 48×40（历史）。owl：齿轮→设置抽屉、
  时钟→全屏历史；Steps/Guidance/Seed/Sampler 参数行移动端不展示。
- **历史**：时钟按钮开全屏覆盖层，两列瀑布流（图片 176 宽、列距 4、纵横比 3:4），
  头部 History + ✕，底部 lock/download-all/delete 三钮。
- **汉堡菜单**：触屏无 hover——按 `(hover: hover)` 门控悬停开启；触屏点按开合、
  点菜单外收起。菜单条目与官方同名（Account Settings / Logout / Quick Start Gallery /
  Director Tools / Usage Limit / Inventory / Tokenizer / Discord），Inventory 与
  Tokenizer owl 未实现→报错提示。
- viewport meta 与官方一致：`maximum-scale=1, user-scalable=no,
  interactive-widget=resizes-content`（键盘弹起压缩内容而非整页缩放）。
