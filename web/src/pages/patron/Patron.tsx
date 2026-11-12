/**
 * 赞助席 `/patron`：独立密码进独立页面。欢迎 + 自己的功能开关，没有后台侧栏。
 */
import { useEffect, useRef, useState } from "preact/hooks";
import { ApiError, getJSON, sendJSON } from "../../lib/api";
import { FEATURE_COPY, FEATURE_FIELDS } from "../../lib/member-policy";
import { reduceMotion } from "../../lib/motion";
import { Badge, Button, Field, Notice, toast } from "../../ui";
import "../../ui/chrome.css";
import "../landing/landing.css";
import { SwitchRow } from "../../ui/Switch";
import "./patron.css";

type Features = Record<(typeof FEATURE_FIELDS)[number], boolean>;

type Me = {
  site_name: string;
  welcome: string;
  queue_jump: boolean;
  self_service: boolean;
  exclusive_account: boolean;
  account_hint: string | null;
  key_fingerprint: string | null;
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
  const [err, setErr] = useState("");
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
      </form>
    </main>
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
      <div class="wrap wrap--narrow">
        <section class="card pt-card" aria-label="赞助席">
          <div class="pt-brand">
            <img src="/_owl/favicon.svg" alt="" width="32" height="32" />
            {me.site_name}
          </div>
          <h1>赞助席</h1>
          <p class="pt-lead">{me.welcome}</p>
          <div class="pt-status">
            <Badge tone={me.queue_jump ? "ok" : "info"}>{me.queue_jump ? "插队已开" : "插队未开"}</Badge>
            <Badge tone={me.exclusive_account ? "accent" : "info"}>
              {me.exclusive_account ? "专属账号" : "走全站共用账号"}
            </Badge>
          </div>
          {me.account_hint && <Notice tone="warn">{me.account_hint}</Notice>}
          {me.key_fingerprint && <p class="pt-meta">Key {me.key_fingerprint}</p>}
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
