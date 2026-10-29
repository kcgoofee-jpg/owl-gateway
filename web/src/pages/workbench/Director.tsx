/**
 * 导演页：网关 /ai/augment-image。上色和表情带 prompt 与 defry；表情写进 prompt。
 */
import { useState } from "preact/hooks";
import { toast } from "../../ui";
import { makeShot, postTool, type Shot } from "./api";
import { readImageFile } from "./files";
import { reloadMe } from "./store";
import { DIRECTOR_EMOTIONS, DIRECTOR_TOOLS, directorRequest, type SourceImage } from "./workbench-config";

export function DirectorPane(props: { hidden: boolean; apiKey: string; addShots: (shots: Shot[]) => void }) {
  const [tool, setTool] = useState("lineart");
  const [prompt, setPrompt] = useState("");
  const [emotion, setEmotion] = useState("neutral");
  const [defry, setDefry] = useState(0);
  const [source, setSource] = useState<SourceImage | null>(null);
  const [shot, setShot] = useState<Shot | null>(null);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const current = DIRECTOR_TOOLS.find((t) => t.id === tool) ?? DIRECTOR_TOOLS[0];

  const run = async () => {
    if (!source || running || !current) return;
    setRunning(true);
    setError(null);
    const controller = new AbortController();
    try {
      const imgs = await postTool(
        "/ai/augment-image",
        directorRequest({ image: source.b64, tool, prompt, emotion, defry }),
        props.apiKey,
        controller.signal,
      );
      const first = imgs[0];
      if (!first) throw new Error("返回里没有图片。");
      const next = makeShot(first, {
        model: tool,
        width: 0,
        height: 0,
        steps: 0,
        scale: 0,
        sampler: tool,
        noise: "",
        seed: 0,
        prompt: prompt.trim(),
        negative: "",
        seconds: 0,
      });
      setShot(next);
      props.addShots([next]);
      void reloadMe();
    } catch (e) {
      setError(e instanceof Error ? e.message : "导演工具没有成功。");
    } finally {
      setRunning(false);
    }
  };

  return (
    <section class="nai-shell" role="tabpanel" hidden={props.hidden} aria-label="导演工具">
      <aside class="nai-sidebar wb-side">
        <div class="nai-scroll">
          <fieldset class="nai-row" aria-label="导演工具">
            {DIRECTOR_TOOLS.map((item) => (
              <button
                key={item.id}
                type="button"
                class="nai-chipbtn"
                aria-pressed={tool === item.id}
                onClick={() => setTool(item.id)}
              >
                {item.label}
              </button>
            ))}
          </fieldset>
          <label class="nai-file">
            选择图片
            <input
              type="file"
              accept="image/png,image/jpeg,image/webp"
              onChange={(e) => {
                const file = e.currentTarget.files?.[0];
                e.currentTarget.value = "";
                if (!file) return;
                void readImageFile(file)
                  .then(setSource)
                  .catch((err: unknown) =>
                    toast(err instanceof Error ? err.message : "这张图读不出来。", { error: true }),
                  );
              }}
            />
          </label>
          {source && <img src={source.dataUrl} alt="" width={96} height={96} />}
          {current?.prompt && (
            <>
              {tool === "emotion" && (
                <label class="nai-label">
                  表情
                  <select
                    class="nai-select"
                    value={emotion}
                    onChange={(e) => setEmotion(e.currentTarget.value)}
                  >
                    {DIRECTOR_EMOTIONS.map((item) => (
                      <option key={item.id} value={item.id}>
                        {item.label}
                      </option>
                    ))}
                  </select>
                </label>
              )}
              <label class="nai-label">
                提示词
                <textarea
                  class="nai-textarea"
                  value={prompt}
                  onInput={(e) => setPrompt(e.currentTarget.value)}
                />
              </label>
              <label class="nai-label">
                Defry
                <input
                  class="nai-input"
                  type="number"
                  min={0}
                  max={5}
                  value={defry}
                  onInput={(e) => setDefry(Number(e.currentTarget.value))}
                />
              </label>
            </>
          )}
        </div>
        <footer class="nai-foot">
          <button type="button" class="nai-generate" disabled={!source || running} onClick={() => void run()}>
            {running ? "处理中…" : "开始处理"}
          </button>
          {error && <p class="nai-status nai-status--bad">{error}</p>}
        </footer>
      </aside>
      <section class="nai-main" aria-label="导演结果">
        <div class="wb-shot">
          {shot ? <img src={shot.url} alt="导演工具结果" /> : <p class="nai-empty">选一张图，再选工具。</p>}
        </div>
      </section>
    </section>
  );
}
