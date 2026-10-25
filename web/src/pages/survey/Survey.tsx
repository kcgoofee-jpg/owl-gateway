/**
 * 成员问卷（/survey）。身份来自 Discord 登录会话：先取 /public/me，登录后带着会话 Cookie 提交到 /member/survey，
 * 不再把 Key 放进请求头。
 */
import { useEffect, useState } from "preact/hooks";
import { getJSON } from "../../lib/api";
import { bootData } from "../../lib/boot";
import { Card, Notice, Skeleton } from "../../ui";
import { displayName, isMe } from "../landing/helpers";
import { SiteFooter, Topbar } from "../landing/Topbar";
import type { Me } from "../landing/types";
import "../landing/landing.css";
import "./survey.css";
import { FeedbackForm } from "./FeedbackForm";
import { SurveyForm } from "./SurveyForm";

export function Survey() {
  const boot = bootData<{ me?: unknown }>();
  const [me, setMe] = useState<Me | undefined>(isMe(boot?.me) ? boot.me : undefined);

  useEffect(() => {
    if (me === undefined) {
      getJSON<Me>("/public/me", { cache: "no-store" })
        .then(setMe)
        .catch(() => setMe({ logged_in: false }));
    }
  }, [me]);

  const member = me?.logged_in ? me : null;
  return (
    <>
      <Topbar
        user={
          member
            ? { name: displayName(member), id: member.discord?.id, avatar: member.discord?.avatar }
            : undefined
        }
        loginOpen={!member}
      />
      <main class="page sv-main">
        <header class="sv-head">
          <h1>成员问卷</h1>
          <p class="muted">大约 1 分钟。你的回答只有站长能看到，用于决定站里下一步优化什么。</p>
        </header>
        {me === undefined ? (
          <div class="stack" aria-busy="true">
            <span class="sr-only">正在读取…</span>
            <Skeleton h="120px" />
            <Skeleton h="220px" />
          </div>
        ) : !member ? (
          <Card title="请先登录">
            <div class="stack">
              <p>问卷用你的 Discord 登录身份识别，一人一份。</p>
              <div>
                <a
                  class="btn btn--primary btn--lg"
                  href="/login"
                  onClick={() => {
                    try {
                      // 登录回调只会回到首页：首页看到这个标记会把人送回问卷
                      sessionStorage.setItem("owl-next", "/survey");
                    } catch {
                      /* 隐私模式：登录后自己再点进来 */
                    }
                  }}
                >
                  用 Discord 登录
                </a>
              </div>
            </div>
          </Card>
        ) : (
          <div class="stack">
            {member.has_key ? (
              <SurveyForm me={member} />
            ) : (
              <Notice>你还没有 Key，所以暂时不能填写使用体验问卷；仍可以在下面提交反馈。</Notice>
            )}
            <FeedbackForm />
          </div>
        )}
      </main>
      <SiteFooter note="问卷结果只有站长可见" />
    </>
  );
}
