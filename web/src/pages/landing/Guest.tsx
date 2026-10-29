import type { ComponentChildren } from "preact";
import { Card, Tabs } from "../../ui";
import { useStore } from "./ctx";
import { DiscordMark } from "./icons";
import { UsageRules } from "./UsageRules";

/* ---------------- 加入服务器（登录了、还没有 Key） ---------------- */

/**
 * 登录了但还没有 Key：只给一个加入服务器的入口。身份组是在别的服务器核验的，登录过了不代表已经在
 * 猫头鹰的服务器里；进了服务器，bot 会带着领 Key。注册关闭时按钮置灰成「关闭注册」，没配邀请链接时给一句说明。
 */
export function JoinServerCard() {
  const { meta } = useStore();
  const m = meta.value;
  const invite = m.discord_invite ?? "";
  const closed = m.registration?.open === false;
  let action: ComponentChildren;
  if (closed) {
    action = (
      <button type="button" class="btn btn--discord btn--lg" disabled title="注册已关闭，开放时间看站长公告">
        关闭注册
      </button>
    );
  } else if (/^https:\/\//i.test(invite)) {
    action = (
      <a class="btn btn--discord btn--lg" href={invite} target="_blank" rel="noopener noreferrer">
        <DiscordMark />
        加入 Discord
      </a>
    );
  } else {
    action = <p class="muted small">用站长公告里的邀请链接加入服务器。</p>;
  }
  return (
    <Card title="加入 Discord 服务器" class="lp-join">
      <div class="lp-join__row">
        <p class="lp-join__text">
          {closed ? (
            <>
              <span class="lp-nb">暂未开放领取 Key，</span>
              <span class="lp-nb">重新开放时会在 Discord 服务器公告。</span>
            </>
          ) : (
            <>
              <span class="lp-nb">Key 在猫头鹰公益站的 Discord 服务器里领：</span>
              <span class="lp-nb">
                加入后在任意频道输入 <code>/register</code>，
              </span>
              <span class="lp-nb">bot 会把 Key 发给你。</span>
            </>
          )}
        </p>
        {action}
      </div>
    </Card>
  );
}

/* ---------------- 客户端设置 ---------------- */

function Guides() {
  const { facts } = useStore();
  const cycle = facts.value.cycle;
  const ms = cycle > 0 ? Math.round(cycle * 1000) : 0;
  return (
    <Card class="reveal">
      <Tabs
        label="使用指南"
        tabs={[
          {
            id: "baibai",
            label: "柏宝绘",
            panel: () => (
              <div class="stack">
                <ol class="lp-list">
                  <li>打开「渠道」→「配置」</li>
                  <li>点击「新建接入点」</li>
                  <li>
                    <span class="lp-nb">「接口地址」填接入地址</span> <code>{location.origin}</code>
                  </li>
                  <li>「API Key」填写你的 Key</li>
                  <li>接入地址填写站点根地址；连接测试只验证基础连接，不代表所有模型或功能均可用</li>
                  <li>“同时出图数”控制并发，“每次请求图片数”是另一项参数；本站每次最多生成一张</li>
                  <li>V5 不支持 Vibe 参考图；请求历史中的“已取消”表示尚未派发就断开</li>
                </ol>
              </div>
            ),
          },
          {
            id: "chatu8",
            label: "智绘姬",
            panel: () => (
              <div class="stack">
                <p class="muted small">SillyTavern 生图插件 st-chatu8。</p>
                <ol class="lp-list">
                  <li>
                    「站点」选<b>其他站点</b>，接口地址填<b>完整地址</b>（接口地址后拼{" "}
                    <code>/ai/generate-image</code>，带路径，<b>不要只填根地址</b>
                    ）——只填根地址的话，遇到限流插件会多发一个无效请求。
                  </li>
                  <li>
                    「生图间隔时间」从默认 100ms 调大
                    {ms ? (
                      <>
                        到 <b>{ms} 以上</b>（毫秒）——本站每个账号约 {cycle} 秒出一张，默认值会连续撞限流。
                      </>
                    ) : (
                      <>——本站每张出完都要歇几秒，默认值会连续撞限流。</>
                    )}
                  </li>
                  <li>
                    一条回复尽量只放 1~2 个 image 标签；「自动点击生成」「永久自动点击」保持关闭（默认已关）。
                  </li>
                  <li>
                    「云端队列」保持<b>关闭</b>即可——本站自己排队，不用插件再排一次。
                    要开的话「队列服务地址」必须一并填（地址以插件作者的说明为准）：
                    留空时请求会发到不存在的队列接口，直接返回 404。
                  </li>
                </ol>
              </div>
            ),
          },
          { id: "rules", label: "使用须知", panel: () => <Rules /> },
        ]}
      />
    </Card>
  );
}

/* ---------------- 使用须知 ---------------- */

/** 括号里的补充说明不拆开：窄屏换行只发生在句子之间，不会把「（30 天后自动删除）」折成两行 */
function KeepParens(props: { text: string }) {
  return (
    <>
      {props.text
        .split(/(（[^）]*）)/)
        .filter(Boolean)
        .map((s, i) =>
          s.startsWith("（") ? (
            <span key={`${i}-${s}`} class="lp-nb">
              {s}
            </span>
          ) : (
            s
          ),
        )}
    </>
  );
}

function Rule(props: { k: string; children: ComponentChildren }) {
  return (
    <div class="lp-rule">
      <dt>{props.k}</dt>
      <dd>{props.children}</dd>
    </div>
  );
}

function Rules() {
  const { meta } = useStore();
  const m = meta.value;
  const extra = m.limits?.key_image_queue ?? 0;
  return (
    <dl class="lp-rules">
      <Rule k="服务">
        非商业公益服务，不保证持续可用；不保存提示词或生成图，请求结果和额度记录仅用于排障与防滥用。
      </Rule>
      <Rule k="规格">
        可用模型和功能以登录后显示的个人权限为准。免费尺寸不超过 1024×1024（或等像素），步数不超过 28。Vibe
        要消耗 Anlas，并受个人额度限制。
      </Rule>
      <Rule k="额度">
        <KeepParens text="模型权限和额度因人而异，以登录后当前信息为准；每日 7 点重置。V5 受本站开放策略及可用账号资源影响，可能暂时不可用或暂停。" />
      </Rule>
      <Rule k="出图节奏">
        排队会动态调度，近期使用较少的成员优先；等待较久的请求顺位会逐步提高，避免久候。预计时间随队列和账号资源变化，不承诺固定名次或时长。每把
        Key 同时只能生成 1 张{extra > 0 ? `，最多再排 ${extra} 张` : ""}
        。被拒后请不要连点重试：短时间内被拒太多次，Key 会被暂时暂停。
      </Rule>
      <Rule k="常用命令">
        在 <span class="lp-nb">Discord 服务器</span>里用 <code>/register</code> 领 Key、
        <code>/resetkey</code> 换新 Key；额度和用量在本页登录后查看。
      </Rule>
      <Rule k="一人一把">
        Key 仅限本人使用，共用或转售会被回收；退出 <span class="lp-nb">Discord 服务器</span>后 Key
        也会被回收。
      </Rule>
      <Rule k="禁止行为">
        <UsageRules />
      </Rule>
    </dl>
  );
}

export function UsageNoticeCard() {
  return (
    <Card title="使用须知" class="reveal">
      <Rules />
    </Card>
  );
}

export function GuestGuides() {
  return <Guides />;
}
