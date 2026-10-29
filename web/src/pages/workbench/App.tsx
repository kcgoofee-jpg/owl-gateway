/**
 * 生图工作台：对照 novelai.net/image 的官方生图页壳子（中文 + 本站图标）。
 * 数据层在 workbench-config.ts，网络在 api.ts。首页和后台不走这套皮。
 */
import { useEffect, useState } from "preact/hooks";
import { DEFAULT_FORM, entryProblem, modelOf, type PaintForm } from "../../lib/nai-config";
import { Button, Card, Dialog, Notice, Skeleton } from "../../ui";
import type { MeMember } from "../landing/types";
import type { Shot } from "./api";
import { BatchPane } from "./Batch";
import { DirectorPane } from "./Director";
import { DrawPane, type DrawState } from "./Draw";
import { MenuIcon, PlusIcon } from "./Icons";
import { loadPrefs, savePrefs, type WbPrefs } from "./prefs";
import { SettingsModal, type SettingsTab } from "./Settings";
import { me as meSignal, start } from "./store";
import { type BatchGroup, type BatchUnified, defaultGroups, type Strings } from "./workbench-config";
import "./workbench.css";

type PageId = "draw" | "director" | "batch";

const INITIAL_DRAW: DrawState = {
  form: { ...DEFAULT_FORM, steps: 23, scale: 5 },
  qualityPreset: "standard",
  ucPreset: "heavy",
  mode: "generate",
  artist: "",
  source: null,
  mask: null,
  strength: 0.7,
  i2iNoise: 0,
  refs: [],
  precise: [],
  characters: [],
  varietyPlus: false,
  suggest: { busy: false, tags: [] },
};

export function WorkbenchApp() {
  useEffect(() => start(), []);
  const me = meSignal.value;
  if (me === undefined) {
    return (
      <div class="wb wb--loading">
        <Card title="生图工作台">
          <Skeleton h="2em" /> <Skeleton w="60%" h="2em" />
        </Card>
      </div>
    );
  }
  if (!me.logged_in) {
    return (
      <div class="wb wb--loading">
        <Card title="生图工作台（Beta）" sub="这里是成员的网页出图界面。">
          <Notice tone="warn">先登录再看这里——工作台按你的 Key 出图和记账。</Notice>
          <p>
            <a class="btn btn--primary" href="/login">
              用 Discord 登录
            </a>
          </p>
        </Card>
      </div>
    );
  }
  return <Workbench me={me} />;
}

function Workbench({ me }: { me: MeMember }) {
  const paid = me.policy?.paid_parameters !== false;
  const blocked = entryProblem(me);
  const [page, setPage] = useState<PageId>("draw");
  const [menuOpen, setMenuOpen] = useState(false);
  const [draw, setDraw] = useState<DrawState>(() => ({ ...INITIAL_DRAW }));
  const patch = (p: Partial<DrawState>) => setDraw((d) => ({ ...d, ...p }));
  const patchForm = (p: Partial<PaintForm>) => setDraw((d) => ({ ...d, form: { ...d.form, ...p } }));
  const [history, setHistory] = useState<Shot[]>(() => {
    if (!loadPrefs().persistHistory) return [];
    try {
      const raw = localStorage.getItem("owl-wb-shots");
      return raw ? (JSON.parse(raw) as Shot[]).slice(0, 8) : [];
    } catch {
      return [];
    }
  });
  const [historyOpen, setHistoryOpen] = useState(false);
  const [mobileResult, setMobileResult] = useState(false);
  const [viewing, setViewing] = useState<Shot | null>(null);
  const [shared, setShared] = useState<Strings>({ artist: "", quality: "", negative: "" });
  const [unified, setUnified] = useState<BatchUnified>({ artist: true, quality: true, negative: true });
  const [groups, setGroups] = useState<BatchGroup[]>(defaultGroups);
  const [prefs, setPrefsState] = useState<WbPrefs>(loadPrefs);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [settingsTab, setSettingsTab] = useState<SettingsTab>("theme");
  useEffect(() => {
    if (!menuOpen) return;
    const close = () => setMenuOpen(false);
    window.addEventListener("click", close);
    return () => window.removeEventListener("click", close);
  }, [menuOpen]);
  const setPrefs = (next: WbPrefs) => {
    setPrefsState(next);
    savePrefs(next);
    if (!next.persistHistory) {
      try {
        localStorage.removeItem("owl-wb-shots");
      } catch {
        /* ignore */
      }
    }
  };
  const addShots = (shots: Shot[]) =>
    setHistory((h) => {
      const cap = prefs.persistHistory ? 8 : 20;
      const next = [...shots, ...h].slice(0, cap);
      if (prefs.persistHistory) {
        try {
          localStorage.setItem("owl-wb-shots", JSON.stringify(next));
        } catch {
          /* 配额满就只留在内存 */
        }
      }
      return next;
    });

  if (blocked) {
    return (
      <div class="wb wb--loading">
        <Card title="生图工作台（Beta）">
          <Notice tone="warn">{blocked}</Notice>
          <p>
            <a class="btn" href="/">
              回首页
            </a>
          </p>
        </Card>
      </div>
    );
  }

  const today = me.today;
  const quota = today
    ? `今日 ${today.images}/${today.daily_images} 张 · V5 ${today.v5}/${today.daily_v5}`
    : null;
  const augment = me.policy?.augment === true;

  const go = (id: PageId) => {
    setPage(id);
    setMenuOpen(false);
  };

  return (
    <div class={prefs.reducedMotion ? "nai-app is-quiet" : "nai-app"} data-wb-theme={prefs.theme}>
      <header class="nv-top">
        <a class="nv-mark" href="/" aria-label="首页">
          <span class="nv-eyes" aria-hidden="true">
            <i />
            <i />
          </span>
          <h1 class="sr-only">生图工作台</h1>
        </a>
        <span class="nv-sr">Beta</span>
        <span class="nv-sr">{quota}</span>
        <span class="nv-top__spacer" />
        <div class="nv-anlas-group">
          <span class="nv-anlas wb-quota">
            Anlas: <b>{me.today?.anlas ?? 0}</b>
          </span>
          <button
            type="button"
            class="nv-plus"
            aria-label="额度说明"
            title={quota ?? "今日额度"}
            onClick={() => {
              setSettingsTab("account");
              setSettingsOpen(true);
            }}
          >
            <PlusIcon />
          </button>
        </div>
        <button
          type="button"
          class="nv-menu"
          aria-label="菜单"
          aria-expanded={menuOpen}
          onClick={(e) => {
            e.stopPropagation();
            setMenuOpen((v) => !v);
          }}
        >
          <MenuIcon />
        </button>
      </header>
      <nav class="nai-drawer nai-nav" aria-label="工作台菜单" hidden={!menuOpen}>
        <span class="nv-who">
          {displayName(me)}
          <i>成员</i>
        </span>
        <p class="nv-nav__sec">账号</p>
        <button
          type="button"
          onClick={() => {
            setSettingsTab("account");
            setSettingsOpen(true);
            setMenuOpen(false);
          }}
        >
          账号设置
        </button>
        <button
          type="button"
          class="nv-nav__out"
          onClick={() => {
            void fetch("/logout", { method: "POST" }).finally(() => {
              location.href = "/";
            });
          }}
        >
          退出
        </button>
        <p class="nv-nav__sec">其他</p>
        <button type="button" role="tab" aria-selected={page === "draw"} onClick={() => go("draw")}>
          绘图
        </button>
        {augment && (
          <button type="button" role="tab" aria-selected={page === "director"} onClick={() => go("director")}>
            导演工具
          </button>
        )}
        <button type="button" role="tab" aria-selected={page === "batch"} onClick={() => go("batch")}>
          批量
        </button>
      </nav>
      <DrawPane
        hidden={page !== "draw"}
        apiKey={me.key ?? ""}
        policyImg2img={me.policy?.img2img === true}
        policyVibe={me.policy?.vibe === true}
        policyUpscale={me.policy?.upscale === true}
        draw={draw}
        patch={patch}
        patchForm={patchForm}
        paid={paid}
        v5Open={me.policy?.v5 === true}
        tagsOpen={me.policy?.tags === true}
        historyOpen={prefs.hideHistory ? false : historyOpen}
        onToggleHistory={() => {
          if (prefs.hideHistory) return;
          setHistoryOpen((v) => !v);
        }}
        shots={history}
        addShots={addShots}
        openShot={setViewing}
        mobileResult={mobileResult}
        setMobileResult={setMobileResult}
        autoDownload={prefs.autoDownload}
        hideHistory={prefs.hideHistory}
        transparentBg={prefs.transparentBg}
        setTransparentBg={(v) => setPrefs({ ...prefs, transparentBg: v })}
      />
      {augment && <DirectorPane hidden={page !== "director"} apiKey={me.key ?? ""} addShots={addShots} />}
      <BatchPane
        hidden={page !== "batch"}
        base={draw.form}
        presets={{ qualityPreset: draw.qualityPreset, ucPreset: draw.ucPreset }}
        mode={draw.mode}
        paid={paid}
        apiKey={me.key ?? ""}
        shared={shared}
        setShared={setShared}
        unified={unified}
        setUnified={setUnified}
        groups={groups}
        setGroups={setGroups}
        addShots={addShots}
        openShot={setViewing}
      />
      <SettingsModal
        open={settingsOpen}
        tab={settingsTab}
        setTab={setSettingsTab}
        prefs={prefs}
        setPrefs={setPrefs}
        quota={quota}
        who={displayName(me)}
        onClose={() => setSettingsOpen(false)}
      />
      <ShotDialog
        shot={viewing}
        onClose={() => setViewing(null)}
        onReuse={(seed) => {
          patchForm({ seed: String(seed) });
          setPage("draw");
          setViewing(null);
        }}
      />
    </div>
  );
}

function ShotDialog(props: { shot: Shot | null; onClose: () => void; onReuse: (seed: number) => void }) {
  const shot = props.shot;
  return (
    <Dialog
      open={shot !== null}
      title="成图详情"
      onClose={props.onClose}
      wide
      footer={
        <>
          {shot && (
            <>
              <a class="btn" href={shot.url} download={`owl-${shot.attrs.seed}.${shot.ext}`}>
                下载
              </a>
              <Button variant="ghost" onClick={() => props.onReuse(shot.attrs.seed)}>
                用这颗种子再画
              </Button>
            </>
          )}
          <Button variant="ghost" onClick={props.onClose}>
            关闭
          </Button>
        </>
      }
    >
      {shot && (
        <div class="wb-dialog">
          <img src={shot.url} alt={`成图 ${shot.attrs.seed}`} />
          <dl class="wb-attrs">
            <div>
              <dt>模型</dt>
              <dd>{modelOf(shot.attrs.model).label}</dd>
            </div>
            <div>
              <dt>尺寸</dt>
              <dd>
                {shot.attrs.width}×{shot.attrs.height}
              </dd>
            </div>
            <div>
              <dt>步数 / 引导</dt>
              <dd>
                {shot.attrs.steps} / {shot.attrs.scale}
              </dd>
            </div>
            <div>
              <dt>采样器</dt>
              <dd>{shot.attrs.sampler}</dd>
            </div>
            <div>
              <dt>种子</dt>
              <dd>{shot.attrs.seed}</dd>
            </div>
            <div>
              <dt>耗时</dt>
              <dd>{shot.attrs.seconds.toFixed(1)} 秒</dd>
            </div>
            <div class="wb-attr--wide">
              <dt>提示词</dt>
              <dd>{shot.attrs.prompt}</dd>
            </div>
            {shot.attrs.negative && (
              <div class="wb-attr--wide">
                <dt>负面</dt>
                <dd>{shot.attrs.negative}</dd>
              </div>
            )}
          </dl>
        </div>
      )}
    </Dialog>
  );
}

function displayName(me: MeMember): string {
  return me.discord?.display_name ?? me.discord?.username ?? "成员";
}
