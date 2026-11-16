/**
 * 占位页：其余 10 个分类先保留侧栏入口与页头（标题 / 描述沿用现有文案），
 * 内容区给出「本页正在接线」的状态说明，后续任务逐页替换成真实表单。
 */
import { PageHeader } from "../controls";
import type { PageMeta } from "../schema";

export function PlaceholderPage({ meta }: { meta: PageMeta }) {
  return (
    <>
      <PageHeader title={meta.title} desc={meta.desc} />
      <div className="wset-placeholder">
        <span className="wset-placeholder__badge">本页正在接线</span>
        <p className="wset-placeholder__text">
          这一页的设置项还在接入 settings.json；接好后会沿用同一套草稿态、校验与「保存并应用」。
          当前可以先在「外观与交互」「排序」「全局热键」「快捷操作」里调整。
        </p>
      </div>
    </>
  );
}
