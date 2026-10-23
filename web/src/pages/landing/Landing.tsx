/**
 * 成员首页（/）。三种人看到三种首屏：
 * - 游客：只有状态区（内含「用 Discord 登录」），下面客户端设置、使用须知；
 * - 有 Key 的成员：今天的额度圆环，下面状态区、重置券和请求记录；
 * - 登录了还没有 Key：加入服务器的入口排最前，状态区占满整行。
 * 开屏每天播一次，见 Splash.tsx。
 */
import { useEffect, useState } from "preact/hooks";
import { Dialog, Skeleton, toast } from "../../ui";
import { SurveyForm } from "../survey/SurveyForm";
import { StoreCtx } from "./ctx";
import { Gallery } from "./Gallery";
import { Grain } from "./Grain";
import { GuestGuides, JoinServerCard } from "./Guest";
import { displayName } from "./helpers";
import { Coupons } from "./Member";
import { MyRequestsCard } from "./Requests";
import { QuotaRings } from "./Rings";
import { Splash, type Landing as SplashLanding, splashActive } from "./Splash";
import { StatusCard } from "./StatusCard";
import { createStore, type Store } from "./store";
import { SiteFooter, Topbar } from "./Topbar";
import type { MeMember } from "./types";
import "./landing.css";

/** 登录时核验身份组（后台设了限定身份组）回到首页带的结果；no 的文案要带上身份组名称。 */
function verifyNotice(v: string, role: string): { text: string; error: boolean } | undefined {
  switch (v) {
    case "ok":
      return {
        text: "身份组验证通过。现在回到猫头鹰公益站的 Discord 服务器，在任意频道输入 /register 领取 Key。",
        error: false,
      };
    case "no":
      return {
        text: `没有检测到「${role}」，暂时不能领取 Key。请先获得该身份组后再来。`,
        error: true,
      };
    case "error":
      return { text: "Discord 暂时无法核验，请稍后再试。", error: true };
    default:
      return undefined;
  }
}

/** /login/callback 回到首页时带的结果（?login=failed 或 ?verify=ok|no|error）：提示一下并把查询参数从地址栏去掉。 */
function useLoginNotice(role: string, metaReady: boolean) {
  const [waited, setWaited] = useState(false);
  useEffect(() => {
    const params = new URLSearchParams(location.search);
    const v = params.get("login");
    const verify = params.get("verify");
    if (!v && !verify) return;
    // 「没有身份组」的提示要带名称：站点信息还没到就先等（最多 3 秒，之后用通用说法）
    if (verify === "no" && !metaReady && !waited) {
      const t = setTimeout(() => setWaited(true), 3000);
      return () => clearTimeout(t);
    }
    if (v === "failed") {
      toast(
        "Discord 登录没有成功。请确认你已加入猫头鹰公益站的 Discord 服务器，然后再点一次「用 Discord 登录」。",
        {
          error: true,
          ms: 9000,
        },
      );
    }
    const n = verify ? verifyNotice(verify, role) : undefined;
    if (n) toast(n.text, { error: n.error, ms: 9000 });
    params.delete("login");
    params.delete("verify");
    const qs = params.toString();
    history.replaceState(null, "", `${location.pathname}${qs ? `?${qs}` : ""}${location.hash}`);
  }, [metaReady, waited, role]);
}

/** 问卷页让未登录的人先登录：登录回调只会回到首页，这里把人送回去。 */
function useReturnToSurvey(loggedIn: boolean) {
  useEffect(() => {
    if (!loggedIn) return;
    try {
      if (sessionStorage.getItem("owl-next") === "/survey") {
        sessionStorage.removeItem("owl-next");
        location.replace("/survey");
      }
    } catch {
      /* 隐私模式：不跳转 */
    }
  }, [loggedIn]);
}

function Loading() {
  return (
    <div class="wrap lp-body" aria-busy="true">
      <span class="sr-only">正在读取…</span>
      <Skeleton h="220px" />
      <Skeleton h="220px" />
    </div>
  );
}

/** 游客：首屏只有状态区（内含登录按钮），下面是客户端设置、使用须知；全在一个 760px 的列里。 */
function GuestHome() {
  return (
    <>
      <section class="lp-hero">
        <Grain />
        <div class="wrap wrap--narrow">
          <h1 class="sr-only">猫头鹰公益站</h1>
          <StatusCard login />
        </div>
      </section>
      <div class="wrap wrap--narrow lp-body">
        <GuestGuides />
      </div>
    </>
  );
}

/** 有 Key 的成员：首屏是居中的额度圆环；下面 Key、重置券和状态区并排。 */
function MemberHome({ me }: { me: MeMember }) {
  return (
    <>
      <section class="lp-hero lp-hero--rings">
        <Grain />
        <div class="wrap">
          <h1 class="sr-only">猫头鹰公益站 · 我的额度</h1>
          <QuotaRings me={me} />
        </div>
      </section>
      <div class="wrap lp-body">
        <StatusCard class="reveal" />
        {me.coupons && me.coupons.length > 0 && <Coupons coupons={me.coupons} />}
        <MyRequestsCard />
      </div>
    </>
  );
}

/** 登录了但还没有 Key：只放加入服务器的入口，下面是状态区。 */
function NoKeyHome() {
  return (
    <div class="wrap lp-body lp-body--top">
      <h1 class="sr-only">猫头鹰公益站</h1>
      <JoinServerCard />
      <StatusCard />
    </div>
  );
}

/** 开屏的落点：游客 / 没有 Key → Logo 的两只眼睛；有 Key → V4.5、V5 两个圆环（不够两个就回 Logo）。 */
function splashLanding(store: Store): () => Promise<SplashLanding> {
  return () =>
    new Promise((resolve) => {
      const done = () => {
        // 等页面按登录状态画完再量位置
        requestAnimationFrame(() =>
          requestAnimationFrame(() => {
            const me = store.me.value;
            const rings = [...document.querySelectorAll<HTMLElement>(".rings .ring")].slice(0, 2);
            if (me?.logged_in && me.has_key && rings.length === 2) resolve({ targets: rings, rings: true });
            else
              resolve({
                targets: [...document.querySelectorAll<HTMLElement>(".brand__eyes i")],
                rings: false,
              });
          }),
        );
      };
      if (store.me.value !== undefined) return done();
      const stop = store.me.subscribe((v) => {
        if (v === undefined) return;
        queueMicrotask(() => stop());
        done();
      });
    });
}

function Page({ store }: { store: Store }) {
  const me = store.me.value;
  const member = me?.logged_in ? me : null;
  const reg = store.meta.value.registration;
  // 和圆环看的是同一个初值（页面加载时定好），两边不会对「这次播不播开屏」各说各的
  const [splash, setSplash] = useState(() => splashActive.value);
  const [surveyDismissed, setSurveyDismissed] = useState(false);
  const [surveyOpen, setSurveyOpen] = useState(false);
  useEffect(() => {
    if (member?.has_key && member.survey_done === false && !surveyDismissed) setSurveyOpen(true);
    if (!member?.has_key || member.survey_done) setSurveyOpen(false);
  }, [member?.has_key, member?.survey_done, surveyDismissed]);
  useLoginNotice(reg?.role_note || "指定身份组", reg !== undefined);
  useReturnToSurvey(!!member);
  return (
    <>
      <div class="sky" aria-hidden="true" />
      {splash && <Splash land={splashLanding(store)} onDone={() => setSplash(false)} />}
      <Topbar
        user={
          member
            ? {
                name: displayName(member),
                id: member.discord?.id,
                avatar: member.discord?.avatar,
                key: member.key,
              }
            : undefined
        }
        // 首页游客的登录入口在状态区里，顶栏不再放第二个
        loginOpen={false}
        brandHref="/#gallery"
      />
      <main class="lp-main">
        {me === undefined ? (
          <Loading />
        ) : member ? (
          member.has_key ? (
            <MemberHome me={member} />
          ) : (
            <NoKeyHome />
          )
        ) : (
          <GuestHome />
        )}
        {me !== undefined && <Gallery />}
      </main>
      {member?.has_key && (
        <Dialog
          open={surveyOpen}
          title="使用体验问卷"
          onClose={() => {
            setSurveyOpen(false);
            setSurveyDismissed(true);
          }}
          wide
        >
          <SurveyForm
            me={member}
            onDone={() => {
              setSurveyOpen(false);
              setSurveyDismissed(true);
              void store.reloadMe();
            }}
          />
        </Dialog>
      )}
      <SiteFooter survey={!!member?.has_key} />
    </>
  );
}

export function Landing() {
  const [store] = useState(createStore);
  useEffect(() => store.start(), [store]);
  return (
    <StoreCtx.Provider value={store}>
      <Page store={store} />
    </StoreCtx.Provider>
  );
}
