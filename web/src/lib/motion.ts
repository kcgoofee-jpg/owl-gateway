/**
 * 动效的统一入口：只从 motion 引入用到的两个函数；系统开了「减少动态效果」时一律不播。
 * 滚动显影（scroll-driven animations）是纯 CSS，见 ui/chrome.css；页面转场（View Transitions）10-23 删除。
 */
import { animate, stagger } from "motion";

export { animate, stagger };

/** 系统偏好「减少动态效果」：所有动效都关掉。 */
export function reduceMotion(): boolean {
  return typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/** 首页统一的缓动（快出、慢收）。 */
export const EASE_OUT: [number, number, number, number] = [0.22, 1, 0.36, 1];
