/**
 * 表格右缘渐隐开关：主窗口里所有 .db-table-wrap（横滚兜底容器）在
 * 「还能向右滚」时挂 data-scroll-right="true"，dashboard.css 据此给容器
 * 直接子表格启用 mask 右缘渐隐，让最后一列被裁切时看得出可以横滚。
 *
 * mask 画在表格上而不是容器上（容器 mask 会把 ::-webkit-scrollbar 自绘
 * 滚动条一起罩淡），mask 相对表格 border box 定位、随内容滚动，靠同次
 * 计算写入表格的 --fade-x（= 视口右缘在表格坐标系里的 x，即
 * scrollLeft + clientWidth - 表格相对滚动原点的偏移）把透明终点钉在
 * 可视区右缘：横滚时逐次更新，纵滚时 x 不变、渐隐竖带自然贴边；吸顶
 * 表头是表格的子元素，与各行一起渐隐。
 *
 * 更新时机（同步计算，事件源本身已按帧节流）：
 *  - 主区 .db-main 的滚动（scroll 不冒泡，捕获阶段才能收到子容器横滚；
 *    表格横滚发生在 .db-table-wrap 自己身上，.db-main 捕获监听能同时
 *    收到两者）；
 *  - 窗口尺寸变化（阈值两侧翻转、列宽重排）；
 *  - 主区 DOM 变化（切页 / 表格数据刷新 / 行进场动画挂载）。
 *
 * 刻意不用 requestAnimationFrame 合并：WKWebView 在窗口被完全遮挡时
 * （occluded / hidden）暂停 rAF，暂停期间切页或改窗口尺寸会让开关
 * 停在旧值，窗口重新露出前渐隐状态不正确。同步路径只读十来个容器的
 * 几个滚动属性（连续读只强制一次布局），写仅发生在取值变化时，不产生
 * 逐帧布局抖动。
 *
 * 判定：scrollWidth - clientWidth - scrollLeft > 1（1px 容差吸收亚像素
 * 取整）。纯 UI 提示，不碰业务；卸载时清掉监听。
 */
import { useEffect } from "react";

export function useTableScrollFade(): void {
  useEffect(() => {
    const main = document.querySelector(".db-main");
    if (!main) return;

    const update = () => {
      const wraps = document.querySelectorAll<HTMLElement>(".db-table-wrap");
      for (const wrap of wraps) {
        const overflow = wrap.scrollWidth - wrap.clientWidth - wrap.scrollLeft;
        if (overflow > 1) {
          wrap.setAttribute("data-scroll-right", "true");
          // --fade-x：视口右缘在表格坐标系里的 x。表格 rect 随滚动移动，
          // 差值里已含 scrollLeft；表格相对容器有偏移（padding 等）时
          // 一并被扣除。值不变不写，避免无谓的样式重算
          const table = wrap.firstElementChild;
          if (table instanceof HTMLElement) {
            const fadeX =
              wrap.getBoundingClientRect().left +
              wrap.clientWidth -
              table.getBoundingClientRect().left;
            const next = `${Math.round(fadeX * 10) / 10}px`;
            if (table.style.getPropertyValue("--fade-x") !== next) {
              table.style.setProperty("--fade-x", next);
            }
          }
        } else if (wrap.hasAttribute("data-scroll-right")) {
          wrap.removeAttribute("data-scroll-right");
        }
      }
    };

    main.addEventListener("scroll", update, { capture: true, passive: true });
    window.addEventListener("resize", update);
    const observer = new MutationObserver(update);
    observer.observe(main, { childList: true, subtree: true });
    update();

    return () => {
      main.removeEventListener("scroll", update, { capture: true });
      window.removeEventListener("resize", update);
      observer.disconnect();
    };
  }, []);
}
