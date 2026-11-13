/**
 * 赞助席 `/patron`：独立密码进独立页面。欢迎 + 自己的功能开关，没有后台侧栏。
 */
import { useEffect, useRef, useState } from "preact/hooks";
import { ApiError, getJSON, sendJSON } from "../../lib/api";
import { FEATURE_COPY, FEATURE_FIELDS } from "../../lib/member-policy";
import { reduceMotion } from "../../lib/motion";
import { Badge, Button, CopyField, Field, Notice, toast } from "../../ui";
import "../../ui/chrome.css";
import "../landing/landing.css";
import { SwitchRow } from "../../ui/Switch";
import "./patron.css";

type Features = Record<(typeof FEATURE_FIELDS)[number], boolean>;

/** 捐赠 Key 的自助视图：完整 Key 仅在捐赠者本人门户明文显示；网关地址仍不下发。 */
type TokenView = {
  missing?: boolean;
  fingerprint?: string;
  token?: string;
  is_share?: boolean;
  enabled?: boolean;
  admin_disabled?: boolean;
  isolated?: boolean;
  concurrency?: number;
  active?: number;
  fails?: number;
  blocked_for?: number;
  images_today?: number;
  v5_used?: number;
  v5_daily_limit?: number;
  anlas_used_today?: number;
};

type Me = {
  site_name: string;
  welcome: string;
  queue_jump: boolean;
  self_service: boolean;
  portal_mode: "basic" | "full";
  donated_account: boolean;
  account_hint: string | null;
  key_fingerprint: string | null;
  token: TokenView | null;
  features: Features;
};

function Grain() {
  const ref = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const cv = ref.current;
    const cx = cv?.getContext("2d");
    if (!cv || !cx) return;
    const paint = () => {
      const dpr = Math.min(window.devicePixelRatio || 1, 2);
      const w = cv.clientWidth;
      const h = cv.clientHeight;
      cv.width = Math.max(1, Math.floor(w * dpr));
      cv.height = Math.max(1, Math.floor(h * dpr));
      const img = cx.createImageData(cv.width, cv.height);
      const rgb = [33, 32, 28];
      for (let i = 0; i < img.data.length; i += 4) {
        const n = 18 + Math.random() * 22;
        img.data[i] = (rgb[0] ?? 0) + n;
        img.data[i + 1] = (rgb[1] ?? 0) + n;
        img.data[i + 2] = (rgb[2] ?? 0) + n;
        img.data[i + 3] = 255;
      }
      cx.putImageData(img, 0, 0);
    };
    paint();
    const ro = new ResizeObserver(paint);
    ro.observe(cv);
    if (reduceMotion()) cv.style.opacity = "0.05";
    return () => ro.disconnect();
  }, []);
  return (
    <div class="grain" aria-hidden="true">
      <canvas ref={ref} />
    </div>
  );
}

function Login(props: { onOk: () => void }) {
  const [pw, setPw] = useState("");
  const [err, setErr] = useState(() => {
    const flag = new URLSearchParams(location.search).get("login");
    if (flag === "failed") return "Discord 登录失败：这个 Discord 账号没有启用的赞助席。";
    if (flag === "unavailable") return "本站尚未配置 Discord 登录，请用门户密码进入。";
    return "";
  });
  const [pending, setPending] = useState(false);
  return (
    <main class="pt-page">
      <Grain />
      <form
        class="card pt-card"
        onSubmit={async (e) => {
          e.preventDefault();
          setPending(true);
          setErr("");
          try {
            await sendJSON("POST", "/patron/api/login", { password: pw });
            props.onOk();
          } catch (x) {
            setErr(x instanceof Error ? x.message : String(x));
          } finally {
            setPending(false);
          }
        }}
      >
        <div class="pt-brand">
          <img src="/_owl/favicon.svg" alt="" width="32" height="32" />
          猫头鹰公益站
        </div>
        <h1>赞助席</h1>
        <p class="pt-hint">这里只属于你的席位，看不到网关其它部分。</p>
        <input type="text" name="username" autoComplete="username" value="patron" hidden readOnly />
        <Field label="门户密码">
          <input
            class="input"
            type="password"
            name="password"
            autoComplete="current-password"
            required
            value={pw}
            onInput={(e) => setPw((e.target as HTMLInputElement).value)}
          />
        </Field>
        {err && (
          <p class="pt-err" role="alert">
            {err}
          </p>
        )}
        <Button type="submit" variant="primary" size="lg" disabled={pending || !pw}>
          {pending ? "登录中…" : "进入"}
        </Button>
        <Button
          type="button"
          size="lg"
          onClick={() => {
            location.href = "/patron/api/login/discord";
          }}
        >
          用 Discord 登录
        </Button>
        <p class="pt-hint">Discord 登录需席位已绑定你的 Discord 账号。</p>
      </form>
    </main>
  );
}

/** 捐赠 Key 分区：状态与用量人人可见；启停和换 Key 只在完整模式开放。 */
function TokenCard(props: { me: Me; onLogout: () => void; onReload: (me: Me) => void }) {
  const t = props.me.token;
  const [pending, setPending] = useState(false);
  const [newKey, setNewKey] = useState("");
  const [err, setErr] = useState("");
  if (!t) return null;
  const full = props.me.portal_mode === "full" && props.me.self_service;
  const act = async (run: () => Promise<Me>, okText: string): Promise<boolean> => {
    setPending(true);
    setErr("");
    try {
      props.onReload(await run());
      toast(okText);
      return true;
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) props.onLogout();
      else setErr(e instanceof Error ? e.message : String(e));
      return false;
    } finally {
      setPending(false);
    }
  };
  if (t.missing) {
    return (
      <section class="card pt-card" aria-label="捐赠 Key">
        <h2 class="pt-h2">捐赠 Key</h2>
        <Notice tone="warn">绑定的捐赠 Key 已不在令牌池里，请联系站长重新绑定。</Notice>
      </section>
    );
  }
  const blocked = t.blocked_for ?? 0;
  const keyValue = t.token ?? t.fingerprint ?? "";
  return (
    <section class="card pt-card" aria-label="捐赠 Key">
      <h2 class="pt-h2">捐赠 Key</h2>
      <div class="pt-status">
        {t.isolated ? (
          <Badge tone="danger">已隔离</Badge>
        ) : t.admin_disabled ? (
          <Badge tone="info">已停用</Badge>
        ) : (
          <Badge tone="ok">启用中</Badge>
        )}
        {blocked > 0 && <Badge tone="warn">冷却 {blocked}s</Badge>}
        {t.is_share && <Badge tone="accent">拼车网关</Badge>}
      </div>
      {t.isolated && (
        <Notice tone="danger">
          这把 Key 被上游隔离了（可能已失效）。{full ? "可在下方直接更换。" : "请联系站长处理。"}
        </Notice>
      )}
      {blocked > 0 && (
        <Notice tone="warn">上游限流冷却中，约 {blocked} 秒后自动恢复，期间请求会排队或稍后再试。</Notice>
      )}
      {keyValue && (
        <div class="pt-key">
          <span class="pt-key__label">捐赠 Key（明文，仅你本人可见）</span>
          <CopyField value={keyValue} label="捐赠 Key" wrap />
        </div>
      )}
      <dl class="pt-kv">
        <div>
          <dt>并发</dt>
          <dd>
            {t.active ?? 0}/{t.concurrency ?? 1}（进行中/上限）
          </dd>
        </div>
        <div>
          <dt>今日出图</dt>
          <dd>{t.images_today ?? 0} 张</dd>
        </div>
        <div>
          <dt>今日 V5</dt>
          <dd>
            {t.v5_used ?? 0}/{t.v5_daily_limit ?? 0}
            {(t.v5_daily_limit ?? 0) === 0 ? "（未开放）" : ""}
          </dd>
        </div>
        <div>
          <dt>今日 Anlas</dt>
          <dd>{t.anlas_used_today ?? 0}</dd>
        </div>
        <div>
          <dt>连续失败</dt>
          <dd>{t.fails ?? 0} 次</dd>
        </div>
      </dl>
      {full ? (
        <>
          <SwitchRow
            title="启用这把 Key"
            desc="停用后新请求不再走这把 Key；手上在途的图会跑完。"
            checked={!t.admin_disabled}
            disabled={pending}
            onChange={(next) =>
              void act(
                () => sendJSON<Me>("PUT", "/patron/api/me/token/enabled", { enabled: next }),
                next ? "已启用" : "已停用",
              )
            }
          />
          <form
            class="pt-rotate"
            onSubmit={(e) => {
              e.preventDefault();
              void act(
                () => sendJSON<Me>("PUT", "/patron/api/me/token/rotate", { token: newKey.trim() }),
                "已更换捐赠 Key",
              ).then((ok) => {
                if (ok) setNewKey("");
              });
            }}
          >
            <Field label="更换 Key（NAI- 或 pk- 开头，沿用原网关地址，先验活再替换）">
              <input
                class="input"
                type="password"
                autoComplete="off"
                placeholder="粘贴新的拼车 Key"
                value={newKey}
                disabled={pending}
                onInput={(e) => setNewKey((e.target as HTMLInputElement).value)}
              />
            </Field>
            <Button type="submit" disabled={pending || !newKey.trim()}>
              {pending ? "正在验证…" : "验证并更换"}
            </Button>
          </form>
        </>
      ) : (
        <p class="pt-hint">当前席位只能查看状态；需要启停或更换 Key，请联系站长开通完整模式。</p>
      )}
      {err && (
        <p class="pt-err" role="alert">
          {err}
        </p>
      )}
    </section>
  );
}

function Seat(props: { me: Me; onLogout: () => void; onReload: (me: Me) => void }) {
  const { me } = props;
  const [pending, setPending] = useState(false);
  const [current, setCurrent] = useState("");
  const [nextPw, setNextPw] = useState("");
  const save = async (name: (typeof FEATURE_FIELDS)[number], value: boolean) => {
    setPending(true);
    try {
      const body = await sendJSON<Me>("PUT", "/patron/api/me/features", { [name]: value });
      props.onReload(body);
      toast("已保存");
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) props.onLogout();
      else toast(e instanceof Error ? e.message : "保存失败", { error: true });
    } finally {
      setPending(false);
    }
  };
  return (
    <main class="pt-page">
      <Grain />
      <div class="wrap wrap--narrow pt-stack">
        <section class="card pt-card" aria-label="赞助席">
          <div class="pt-brand">
            <img src="/_owl/favicon.svg" alt="" width="32" height="32" />
            {me.site_name}
          </div>
          <h1>赞助席</h1>
          <p class="pt-lead">{me.welcome}</p>
          <div class="pt-status">
            <Badge tone={me.queue_jump ? "ok" : "info"}>{me.queue_jump ? "插队已开" : "插队未开"}</Badge>
            <Badge tone={me.donated_account ? "accent" : "info"}>
              {me.donated_account ? "捐赠账号" : "走全站共用账号"}
            </Badge>
          </div>
          {me.account_hint && <Notice tone="warn">{me.account_hint}</Notice>}
          {me.key_fingerprint && <p class="pt-meta">Key {me.key_fingerprint}</p>}
        </section>
        <TokenCard me={me} onLogout={props.onLogout} onReload={props.onReload} />
        <section class="card pt-card" aria-label="功能与密码">
          <h2 class="pt-h2">功能开关</h2>
          <section class="pt-switches" aria-label="功能开关">
            {FEATURE_FIELDS.map((name) => (
              <SwitchRow
                key={name}
                title={FEATURE_COPY[name].label}
                desc={FEATURE_COPY[name].help}
                checked={me.features[name]}
                disabled={!me.self_service || pending}
                onChange={(next) => void save(name, next)}
              />
            ))}
          </section>
          {!me.self_service && <p class="pt-hint">此席位未开放自行改开关，请联系站长。</p>}
          <form
            class="pt-pw"
            onSubmit={async (e) => {
              e.preventDefault();
              setPending(true);
              try {
                await sendJSON("POST", "/patron/api/me/password", { current, new: nextPw });
                toast("密码已改，请重新登录");
                props.onLogout();
              } catch (err) {
                toast(err instanceof ApiError ? err.message : "改密失败", { error: true });
              } finally {
                setPending(false);
              }
            }}
          >
            <Field label="当前密码">
              <input
                class="input"
                type="password"
                autoComplete="current-password"
                value={current}
                onInput={(e) => setCurrent((e.target as HTMLInputElement).value)}
              />
            </Field>
            <Field label="新密码（至少 12 个字符）">
              <input
                class="input"
                type="password"
                autoComplete="new-password"
                value={nextPw}
                onInput={(e) => setNextPw((e.target as HTMLInputElement).value)}
              />
            </Field>
            <div class="pt-foot">
              <Button type="submit" disabled={pending || current.length === 0 || nextPw.length < 12}>
                修改门户密码
              </Button>
              <span class="spacer" />
              <Button
                variant="ghost"
                disabled={pending}
                onClick={async () => {
                  setPending(true);
                  try {
                    await sendJSON("POST", "/patron/api/logout");
                    props.onLogout();
                  } catch (e) {
                    if (e instanceof ApiError && e.status === 401) props.onLogout();
                    else toast(e instanceof Error ? e.message : "退出失败", { error: true });
                  } finally {
                    setPending(false);
                  }
                }}
              >
                退出
              </Button>
            </div>
          </form>
        </section>
      </div>
    </main>
  );
}

export function Patron() {
  const [me, setMe] = useState<Me | null | false>(null);
  const [loadError, setLoadError] = useState("");
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    let alive = true;
    getJSON<Me>("/patron/api/me")
      .then((data) => {
        if (alive) setMe(data);
      })
      .catch((e: unknown) => {
        if (!alive) return;
        if (e instanceof ApiError && e.status === 401) setMe(false);
        else setLoadError(e instanceof Error ? e.message : String(e));
      });
    return () => {
      alive = false;
    };
  }, [attempt]);
  if (me === false) return <Login onOk={() => location.reload()} />;
  if (!me) {
    return (
      <main class="pt-page">
        <Grain />
        {loadError && (
          <section class="card pt-card">
            <Notice tone="danger">{loadError}</Notice>
            <Button
              onClick={() => {
                setLoadError("");
                setAttempt(attempt + 1);
              }}
            >
              重试
            </Button>
          </section>
        )}
      </main>
    );
  }
  return <Seat me={me} onLogout={() => setMe(false)} onReload={(next) => setMe(next)} />;
}
