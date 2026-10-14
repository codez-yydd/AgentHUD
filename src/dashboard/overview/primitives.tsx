/**
 * 总览页共享的小组件：加载骨架。
 * 卡片容器（Card）与骨架块（Skeleton）是主窗口三页共用原语，已上移到
 * ../primitives（纯搬移，语句未改）；此处保留再导出，既有 `./primitives`
 * 引用保持不变。
 */
import { Skeleton } from "../primitives";

export { Card, Skeleton } from "../primitives";

/** 首屏骨架：指标卡 4 张 + 趋势图 + 热力图 + 表格，形状与真实内容一致，
 *  避免加载完成后布局跳动。 */
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

      <div className="db-card">
        <div className="db-card__head">
          <Skeleton className="h-3.5 w-24" />
        </div>
        <div className="db-card__body">
          <div className="db-heat__grid">
            {Array.from({ length: 14 }, (_, col) => (
              <div key={col} className="db-heat__col">
                {Array.from({ length: 7 }, (_, row) => (
                  <Skeleton key={row} className="db-heat__cell" />
                ))}
              </div>
            ))}
          </div>
        </div>
      </div>

      <div className="db-card">
        <div className="db-card__head">
          <Skeleton className="h-3.5 w-24" />
        </div>
        <div className="db-card__body">
          {[0, 1, 2, 3].map((i) => (
            <Skeleton key={i} className="mt-2 h-6 w-full" />
          ))}
        </div>
      </div>
    </>
  );
}
