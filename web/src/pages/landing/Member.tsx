import { useEffect, useRef, useState } from "preact/hooks";
import { ApiError, sendJSON } from "../../lib/api";
import { scopeText } from "../../lib/format";
import { reduceMotion } from "../../lib/motion";
import { Button, Card, Dialog, toast } from "../../ui";
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
 * 用之前确认一次；用掉后刷新额度，首屏对应的圆环会重新充满（滚回顶部让人看到）。
 */
export function Coupons({ coupons }: { coupons: Coupon[] }) {
  const { reloadMe } = useStore();
  const [target, setTarget] = useState<Coupon | null>(null);
  const [busy, setBusy] = useState(false);
  const last = useRef<Coupon | null>(null);
  const now = useNowSec();
  if (target) last.current = target;

  const use = async () => {
    if (!target) return;
    setBusy(true);
    let ok = false;
    try {
      await sendJSON("POST", `/public/me/coupons/${encodeURIComponent(String(target.id))}/use`);
      ok = true;
      toast(`已重置，今天的 ${scopeText(target.scope)} 额度已恢复。`);
    } catch (e) {
      toast(e instanceof ApiError ? e.message : "没有成功，请稍后再试", { error: true });
    } finally {
      setBusy(false);
      setTarget(null);
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
            <Button variant="primary" class="lp-coupon__use" onClick={() => setTarget(c)}>
              使用
              <span class="sr-only">，剩余有效时间</span>
              <span class="lp-coupon__cd">{countdown(c.expires_at, now)}</span>
            </Button>
          </li>
        ))}
      </ul>
      <Dialog
        open={!!target}
        title="使用重置券"
        onClose={() => !busy && setTarget(null)}
        footer={
          <>
            <Button variant="ghost" onClick={() => setTarget(null)} disabled={busy}>
              取消
            </Button>
            <Button variant="primary" onClick={use} disabled={busy}>
              {busy ? "重置中…" : "确定使用"}
            </Button>
          </>
        }
      >
        <p>
          现在重置今天的 {scopeText(last.current?.scope)} 额度？
          {last.current?.note ? `（「${last.current.note}」）` : ""}
        </p>
        <p class="muted small">一张券只能用一次，用掉后不能撤销。</p>
      </Dialog>
    </Card>
  );
}
