import { useEffect, useRef, useState } from "preact/hooks";
import { ApiError, sendJSON } from "../../lib/api";
import { scopeText } from "../../lib/format";
import { reduceMotion } from "../../lib/motion";
import { Button, Card, toast } from "../../ui";
import { useStore } from "./ctx";
import { countdown } from "./helpers";
import type { Coupon } from "./types";

/* ---------------- 重置券 ---------------- */

/** 每秒走一格的「现在」（秒）。 */
function useNowSec(): number {
  const [now, setNow] = useState(() => Date.now() / 1000);
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now() / 1000), 1000);
    return () => clearInterval(t);
  }, []);
  return now;
}

/**
 * 重置券：卡片只有标题。每张券写重置范围，有备注时多一行；到期倒计时放在「使用」按钮里。
 * 站长 10-24：不再弹确认框，点「使用」直接重置；用掉后刷新额度，首屏对应的圆环会重新充满（滚回顶部让人看到）。
 */
export function Coupons({ coupons }: { coupons: Coupon[] }) {
  const { reloadMe } = useStore();
  const [busy, setBusy] = useState(false);
  // 防连点用 ref：按钮保持可用（disabled 的 primary 按钮在暗色下对比度不达标，axe 会抓）
  const busyRef = useRef(false);
  const now = useNowSec();

  const use = async (coupon: Coupon) => {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    let ok = false;
    try {
      await sendJSON("POST", `/public/me/coupons/${encodeURIComponent(String(coupon.id))}/use`);
      ok = true;
      toast(`已重置，今天的 ${scopeText(coupon.scope)} 额度已恢复。`);
    } catch (e) {
      toast(e instanceof ApiError ? e.message : "没有成功，请稍后再试", { error: true });
    } finally {
      busyRef.current = false;
      setBusy(false);
      await reloadMe();
      if (ok) window.scrollTo({ top: 0, behavior: reduceMotion() ? "auto" : "smooth" });
    }
  };

  if (!coupons.length) return null;
  return (
    <Card title="重置券" class="reveal">
      <ul class="lp-coupons">
        {coupons.map((c) => (
          <li key={c.id} class="lp-coupon">
            <div class="lp-coupon__body">
              <p>重置 {scopeText(c.scope)}</p>
              {c.note && <p class="muted small">「{c.note}」</p>}
            </div>
            <Button variant="primary" class="lp-coupon__use" onClick={() => void use(c)}>
              {busy ? "重置中…" : "使用"}
              <span class="sr-only">，剩余有效时间</span>
              <span class="lp-coupon__cd">{countdown(c.expires_at, now)}</span>
            </Button>
          </li>
        ))}
      </ul>
    </Card>
  );
}
