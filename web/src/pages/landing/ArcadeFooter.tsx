import "./arcade-footer.css";

/**
 * 首页底部的游戏入口。放在 Landing.tsx 的 <main> 之后，三种身份都看得到。
 * 站点是 MPA（多页构建），所以用普通 <a> 走浏览器原生跳转，不要接前端路由。
 *
 * 用法：在 Landing.tsx 里
 *   import { ArcadeFooter } from "./ArcadeFooter";
 *   ...
 *     </main>
 *     <ArcadeFooter />
 *   </>
 */
export function ArcadeFooter() {
  return (
    <footer class="lp-footer">
      <div class="wrap wrap--narrow">
        <a class="lp-footer__card" href="/poker/">
          <span class="lp-footer__badge">游戏厅</span>
          <span class="lp-footer__body">
            <span class="lp-footer__name">猫头鹰牌局</span>
            <span class="lp-footer__desc">无限注德州 · 6 人桌 · 和站里的朋友开一局</span>
          </span>
          <span class="lp-footer__go" aria-hidden="true">
            开局 →
          </span>
        </a>
      </div>
    </footer>
  );
}
