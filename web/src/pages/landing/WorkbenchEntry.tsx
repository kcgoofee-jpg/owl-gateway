/**
 * 生图工作台（Beta）入口卡：整页工作台在 /workbench（pages/workbench），这里只负责引导过去。
 * 入口闸门复用 nai-config 的 entryProblem：没拿到 Key、要网页登录、出图没开放时，按钮换成对应提示。
 */

import { entryProblem } from "../../lib/nai-config";
import { Badge, Card, Notice } from "../../ui";
import type { MeMember } from "./types";

export function WorkbenchEntry({ me }: { me: MeMember }) {
  const problem = entryProblem(me);
  return (
    <Card
      id="workbench-entry"
      class="reveal"
      title={
        <>
          生图工作台 <Badge tone="accent">Beta</Badge>
        </>
      }
      sub="整页的网页出图界面：五组提示词批量、参考图与局部重绘（按你的 Key 能力显示）。"
    >
      {problem ? (
        <Notice tone="warn">{problem}</Notice>
      ) : (
        // div 而不是 p：.lp-main p a 的链接色会盖掉 .btn--primary 的白字（对比度翻车）
        <div class="wb-entry__row">
          <span class="muted small">
            测试期间界面和参数还会调整，欢迎在 Discord 提意见。画布与历史只在当前页面，刷新即清空。
          </span>
          <a class="btn btn--primary btn--lg wb-entry__go" href="/workbench">
            前往工作台
          </a>
        </div>
      )}
    </Card>
  );
}
