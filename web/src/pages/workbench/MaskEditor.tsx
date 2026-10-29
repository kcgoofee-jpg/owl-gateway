/** 局部重绘蒙版：白=重画，黑=保留。画在原图上，导出 PNG。 */
import { useEffect, useRef, useState } from "preact/hooks";
import type { SourceImage } from "./workbench-config";

export function MaskEditor(props: {
  source: SourceImage;
  mask: SourceImage | null;
  width: number;
  height: number;
  onSave: (mask: SourceImage) => void;
  onClose: () => void;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [pen, setPen] = useState(20);
  const [erase, setErase] = useState(false);
  const drawing = useRef(false);
  const last = useRef<{ x: number; y: number } | null>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    canvas.width = props.width;
    canvas.height = props.height;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.fillStyle = "#000";
    ctx.fillRect(0, 0, canvas.width, canvas.height);
    if (!props.mask) return;
    const img = new Image();
    img.onload = () => ctx.drawImage(img, 0, 0, canvas.width, canvas.height);
    img.src = props.mask.dataUrl;
  }, [props.source, props.mask, props.width, props.height]);

  const pos = (e: PointerEvent, canvas: HTMLCanvasElement) => {
    const r = canvas.getBoundingClientRect();
    return {
      x: ((e.clientX - r.left) * canvas.width) / r.width,
      y: ((e.clientY - r.top) * canvas.height) / r.height,
    };
  };

  const stroke = (canvas: HTMLCanvasElement, x: number, y: number) => {
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.lineCap = "round";
    ctx.lineJoin = "round";
    ctx.lineWidth = pen;
    ctx.strokeStyle = erase ? "#000" : "#fff";
    ctx.beginPath();
    const prev = last.current;
    if (prev) ctx.moveTo(prev.x, prev.y);
    else ctx.moveTo(x, y);
    ctx.lineTo(x, y);
    ctx.stroke();
    last.current = { x, y };
  };

  const save = () => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const dataUrl = canvas.toDataURL("image/png");
    props.onSave({ name: "mask.png", dataUrl, b64: dataUrl.slice(dataUrl.indexOf(",") + 1) });
  };

  return (
    <div class="nv-mask" role="dialog" aria-label="蒙版画笔" aria-modal="true">
      <header class="nv-mask__bar">
        <span>画笔 {pen}</span>
        <input
          type="range"
          min={4}
          max={80}
          value={pen}
          aria-label="笔刷大小"
          onInput={(e) => setPen(Number(e.currentTarget.value))}
        />
        <button type="button" aria-pressed={!erase} onClick={() => setErase(false)}>
          画
        </button>
        <button type="button" aria-pressed={erase} onClick={() => setErase(true)}>
          擦
        </button>
        <button
          type="button"
          onClick={() => {
            const canvas = canvasRef.current;
            const ctx = canvas?.getContext("2d");
            if (!canvas || !ctx) return;
            ctx.fillStyle = "#000";
            ctx.fillRect(0, 0, canvas.width, canvas.height);
          }}
        >
          清空
        </button>
        <span class="nv-mask__grow" />
        <button type="button" class="nai-generate nv-mask__save" onClick={save}>
          保存并关闭
        </button>
        <button type="button" class="nv-iconbtn" aria-label="关闭" onClick={props.onClose}>
          ×
        </button>
      </header>
      <div
        class="nv-mask__stage"
        style={{
          backgroundImage: `url(${props.source.dataUrl})`,
          aspectRatio: `${props.width} / ${props.height}`,
        }}
      >
        <canvas
          ref={canvasRef}
          class="nv-mask__paint"
          onPointerDown={(e) => {
            const canvas = canvasRef.current;
            if (!canvas) return;
            canvas.setPointerCapture(e.pointerId);
            drawing.current = true;
            last.current = null;
            const p = pos(e as unknown as PointerEvent, canvas);
            stroke(canvas, p.x, p.y);
          }}
          onPointerMove={(e) => {
            if (!drawing.current) return;
            const canvas = canvasRef.current;
            if (!canvas) return;
            const p = pos(e as unknown as PointerEvent, canvas);
            stroke(canvas, p.x, p.y);
          }}
          onPointerUp={() => {
            drawing.current = false;
            last.current = null;
          }}
        />
      </div>
    </div>
  );
}
