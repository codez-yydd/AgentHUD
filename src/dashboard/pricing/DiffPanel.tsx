/**
 * 价格更新差异面板（大窗版）：内置参考表 / models.dev 在线同步的差异结果，
 * 逐条勾选后应用到价格表（绝不自动覆盖未勾选项）。行为与窄窗 PricingPanel
 * 的差异卡完全一致，只是换成大窗卡片与宽行布局。
 */
import type { PricingDiff } from "../../types";
import { useI18n } from "../../i18n";
import { Card } from "../primitives";
import { fmtStamp, fmtTriplet, scaleCny } from "./helpers";

export function DiffPanel({
  diff,
  source,
  meta,
  selected,
  fxRate,
  applying,
  onToggle,
  onApply,
  onCollapse,
}: {
  diff: PricingDiff;
  /** 差异来源：builtin = 编译期内置表；modelsdev = models.dev 在线官方价 */
  source: "builtin" | "modelsdev";
  /** models.dev 同步的数据元信息（仅在线来源展示） */
  meta: { fetchedAt: number; fromCache: boolean } | null;
  selected: Set<string>;
  fxRate: number;
  applying: boolean;
  onToggle: (key: string) => void;
  onApply: () => void;
  onCollapse: () => void;
}) {
  const { t } = useI18n();

  return (
    <Card
      // 标题只表达来源；版本号只在副标题出现一次。注意 models.dev 来源的
      // version 就是来源标识字符串 "models.dev"（后端 pricing.rs），既不做
      // v 前缀拼接、也不在副标题重复（在线数据时间在正文 meta 行展示）
      title={
        source === "modelsdev"
          ? t("dashboard.pricing.diffTitleModelsdev")
          : t("dashboard.pricing.diffTitle")
      }
      sub={source === "builtin" && diff.version ? `v${diff.version}` : undefined}
      action={
        <div className="db-diff__actions">
          <button type="button" className="db-btn" onClick={onCollapse}>
            {t("dashboard.pricing.collapse")}
          </button>
          <button
            type="button"
            className="db-btn db-btn--primary"
            onClick={onApply}
            disabled={applying || selected.size === 0}
          >
            {applying
              ? t("dashboard.pricing.applying")
              : selected.size > 0
                ? t("dashboard.pricing.applySelectedCount", {
                    count: selected.size,
                  })
                : t("dashboard.pricing.applySelected")}
          </button>
        </div>
      }
    >
      {/* 口径说明（放在正文首行：卡片头副标题是单行省略样式，长文案会截断） */}
      <p className="db-diff__hint">
        {source === "modelsdev"
          ? t("dashboard.pricing.diffHintModelsdev")
          : t("dashboard.pricing.diffHint")}
      </p>

      {/* models.dev 数据时间 + 离线缓存标注（在线同步特有） */}
      {source === "modelsdev" && meta && (
        <p className="db-diff__meta">
          {t("dashboard.pricing.modelsdevFetchedAt", {
            time: fmtStamp(meta.fetchedAt),
          })}
          {meta.fromCache && (
            <span className="db-diff__meta-cache">
              {t("dashboard.pricing.modelsdevFromCache")}
            </span>
          )}
        </p>
      )}

      {/* 无价格模型警示：实际在用但两边都没价格，花费按 0 计 */}
      {diff.missing.length > 0 && (
        <div className="db-diff__warn">
          <div className="db-diff__warn-title">
            {t("dashboard.pricing.missingWarn", { count: diff.missing.length })}
          </div>
          <ul className="db-diff__warn-list">
            {diff.missing.map((model) => (
              <li key={model} className="num">
                {model}
              </li>
            ))}
          </ul>
          <div className="db-diff__warn-hint">
            {t("dashboard.pricing.addPriceBelow")}
          </div>
        </div>
      )}

      <div className="db-diff__list">
        {/* 新增模型（参考有、用户未配置） */}
        {diff.new_models.map((item) => {
          const key = item.model_id;
          return (
            <label key={`new-${key}`} className="db-diff__item">
              <input
                type="checkbox"
                checked={selected.has(key)}
                onChange={() => onToggle(key)}
                className="db-diff__check"
              />
              <span className="db-tag db-tag--new">
                {t("dashboard.pricing.badgeNew")}
              </span>
              <span className="db-diff__model num">{item.model_id}</span>
              {item.reference_id && (
                <span
                  className="db-tag db-tag--ref"
                  title={t("dashboard.pricing.refFrom", {
                    model: item.reference_id,
                  })}
                >
                  ≈ {item.reference_id}
                </span>
              )}
              <span className="db-diff__price num">
                $ {fmtTriplet(item.default)}
                <span className="db-diff__cny">
                  ≈ ¥ {fmtTriplet(scaleCny(item.default, fxRate))}
                </span>
              </span>
            </label>
          );
        })}

        {/* 价格变动（以 USD 参考价为判定基准） */}
        {diff.changed.map((item) => {
          const key = item.model_id;
          const user = item.user;
          return (
            <label key={`chg-${key}`} className="db-diff__item">
              <input
                type="checkbox"
                checked={selected.has(key)}
                onChange={() => onToggle(key)}
                className="db-diff__check"
              />
              <span className="db-tag db-tag--changed">
                {t("dashboard.pricing.badgeChanged")}
              </span>
              <span className="db-diff__model num">{item.model_id}</span>
              <span className="db-diff__price num">
                <span className="db-diff__old">
                  {t("dashboard.pricing.old")}{" "}
                  {user
                    ? `$ ${fmtTriplet(user)}`
                    : t("dashboard.pricing.unpriced")}
                </span>
                <span className="db-diff__arrow">→</span>
                <span className="db-diff__new">$ {fmtTriplet(item.default)}</span>
                <span className="db-diff__cny">
                  ≈ ¥ {fmtTriplet(scaleCny(item.default, fxRate))}
                </span>
              </span>
            </label>
          );
        })}

        {diff.new_models.length === 0 && diff.changed.length === 0 && (
          <div className="db-empty">{t("dashboard.pricing.noDiff")}</div>
        )}
      </div>
    </Card>
  );
}
