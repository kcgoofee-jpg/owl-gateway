/**
 * 网关在 HTML 里注入的首屏数据：<script type="application/json" id="owl-boot">…</script>
 * （type=application/json 不会被执行，所以不受 CSP script-src 限制）。拿不到就返回 null，页面照常走接口。
 */
export function bootData<T>(): T | null {
  const el = document.getElementById("owl-boot");
  if (!el?.textContent) return null;
  try {
    return JSON.parse(el.textContent) as T;
  } catch {
    return null;
  }
}
