# ui-diff：官网 vs owl 逐控件对比工作台

对照 https://novelai.net/image 与本地 owl 工作台，对同一份控件清单
（12 个起步：导航行/Anlas/模型药丸/提示词标签页/编辑器/Steps/Sampler/
Image Settings/Generate/额度条/右栏/齿轮）做元素截图 + computed style 采集，
自动产出差异报告（宽高/配色/字号/圆角/过渡等 19 项属性逐项对比）。

## 用法

```bash
# 1) 生成两个采集脚本（控件清单改 generate.py 里的 CONTROLS）
python3 tools/ui-diff/generate.py
# 2) 起本地栈：cargo xtask dev + vite --port 5174
# 3) 各跑一次（ego-browser 已登录官网与 owl）
ego-browser nodejs < capture-official.mjs
ego-browser nodejs < capture-owl.mjs
# 4) 出报告
node report.mjs   # → diff-report.md + captures/<target>/<id>.png
```

- 官网 finder 用"最小可见文本元素"定位（官网类名是 styled-components 哈希），
  模型下拉是 react-select（`[class*=singleValue]`）。
- 已知干扰：官网登录账号的支付横幅会顶下布局（+52px），脚本会尝试关掉；
  cookie 横幅可能压住底部区域。
- 报告是阶段 2–8 的规格来源：每修一个控件，重跑此流程核对归零。
