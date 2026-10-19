/**
 * 总览页共享的小组件：加载骨架。
 * 卡片容器（Card）与骨架块（Skeleton）是主窗口三页共用原语，已上移到
 * ../primitives（纯搬移，语句未改）；此处保留再导出，既有 `./primitives`
 * 引用保持不变。
 */
import { Skeleton } from "../primitives";

export { Card, Skeleton } from "../primitives";

/** 首屏骨架：指标卡 4 张 + 趋势图 + 「热力图 + 表格」行（与真实布局同构，
 *  一起包进 .db-overview-duo：默认上下堆叠，仅大窗 ≥1340px 并排两列），
 *  形状与真实内容一致，避免加载完成后布局跳动。 */
export function OverviewSkeleton() {
  return (
    <>
      <div className="db-metrics">
        {[0, 1, 2, 3].map((i) => (
          <div key={i} className="db-card db-metric">
            <Skeleton className="h-3 w-16" />
            <Skeleton className="mt-3 h-7 w-24" />
            <Skeleton className="mt-3 h-3 w-20" />
          </div>
        ))}
      </div>

      <div className="db-card">
        <div className="db-card__head">
          <Skeleton className="h-3.5 w-20" />
        </div>
        <div className="db-card__body">
          <div className="db-skeleton-bars">
            {[42, 66, 35, 78, 52, 88, 61, 46, 72, 58, 40, 84].map((h, i) => (
              <div key={i} className="db-skeleton-bars__col">
                <Skeleton style={{ height: `${h}%` }} />
              </div>
            ))}
          </div>
        </div>
      </div>

      {/* 热力图 + Agent 表骨架：与真实布局同构——同样包一层 .db-overview-duo
          （默认上下堆叠、≥1340px 并排两列），加载完成后行结构不跳变。
          热力图骨架整卡复用真实结构（QuotaMatrixCard「容器复用同一网格模板」
          先例）：52 列真实网格模板 + 星期标签列 + 月份标签行 + 三张统计卡 +
          脚注行，高度随列宽自适应并与真实卡对齐（格子为半透明脉冲占位，
          底色沿用 .db-skeleton，见 .db-skeleton.db-heat__cell）；表格骨架
          5 行占位、行高 h-9 + 间距 mt-2.5 ≈ 真实额度行（百分数行 + 进度条，
          td 上下 padding 10px）约 44px 的行距，5 行取常见 Agent 数中位，
          duo 行总高与加载后接近。 */}
      <div className="db-overview-duo">
        <div className="db-card">
          <div className="db-card__head">
            <Skeleton className="h-3.5 w-24" />
          </div>
          <div className="db-card__body">
            <div className="db-heat">
              <div className="db-heat__chart">
                <div className="db-heat__body">
                  <div className="db-heat__weekdays">
                    {Array.from({ length: 7 }, (_, i) => (
                      <span key={i} className="db-heat__weekday" />
                    ))}
                  </div>
                  <div className="db-heat__grid">
                    {Array.from({ length: 52 }, (_, col) => (
                      <div key={col} className="db-heat__col">
                        {Array.from({ length: 7 }, (_, row) => (
                          <Skeleton key={row} className="db-heat__cell" />
                        ))}
                      </div>
                    ))}
                  </div>
                </div>
                {/* 月份标签行骨架：占位条横跨全部 52 轨（与真实标签行同位同高） */}
                <div className="db-heat__monthrow">
                  <div className="db-heat__monthrow-spacer" />
                  <div className="db-heat__months">
                    <Skeleton
                      className="h-3.5"
                      style={{ gridColumn: "1 / -1" }}
                    />
                  </div>
                </div>
              </div>
              {/* 三张统计卡骨架：复用真实 .db-heat__stat 轮廓（标签 + 大数两行） */}
              <div className="db-heat__aside">
                {[0, 1, 2].map((i) => (
                  <div key={i} className="db-heat__stat">
                    <Skeleton className="h-3.5 w-14" />
                    <Skeleton className="mt-2 h-5 w-20" />
                  </div>
                ))}
              </div>
              {/* 口径脚注骨架：复用 .db-heat__foot 独占整行的布局语义，
                  h-4 = 16px 对齐一行 10px/1.6 小字 */}
              <Skeleton className="db-heat__foot h-4" />
            </div>
          </div>
        </div>

        <div className="db-card">
          <div className="db-card__head">
            <Skeleton className="h-3.5 w-24" />
          </div>
          <div className="db-card__body">
            {[0, 1, 2, 3, 4].map((i) => (
              <Skeleton key={i} className="mt-2.5 h-9 w-full" />
            ))}
          </div>
        </div>
      </div>
    </>
  );
}
