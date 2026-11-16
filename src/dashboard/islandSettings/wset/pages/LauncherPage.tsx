/**
 * 页⑪「剪贴板与启动器」：剪贴板保留条数 + 主页底部启动器列表。
 */
import {
  Note,
  PageHeader,
  PageResetBar,
  RecordEditor,
  Row,
  Section,
  Stepper,
} from "../controls";
import { PAGE_FIELDS, type PageKey } from "../schema";
import {
  readNumber,
  readRecords,
  UnsupportedTag,
  type PageProps,
} from "./common";

const PAGE: PageKey = "launcher";

const LAUNCHER_FIELDS = [
  { key: "name", placeholder: "名称，如 文件资源管理器", width: 190 },
  { key: "path", kind: "path" as const, browse: "any" as const, placeholder: "程序或文件路径", stretch: 1 },
];

const LAUNCHER_NEW_ITEM = { name: "", path: "" };

export function LauncherPage({ draft }: PageProps) {
  return (
    <>
      <PageHeader
        title="剪贴板与启动器"
        desc="剪贴板保留条数与主页底部的应用启动器。"
      />

      <Section title="剪贴板">
        <Row
          title="保留条数"
          desc="只保留最近这么多条；置顶的不计入。"
          error={draft.errors["clipboard.max"]}
          tag={<UnsupportedTag draft={draft} keyName="clipboard.max" />}
          rowKey="clipboard.max"
        >
          <Stepper
            value={readNumber(draft, "clipboard.max", 10)}
            min={1}
            max={100}
            ariaLabel="剪贴板保留条数"
            onChange={(next) => draft.setValue("clipboard.max", next)}
          />
        </Row>
      </Section>

      <Section title="启动器">
        <Row
          wide
          title="应用或文件启动器"
          desc="主页底部默认显示资源管理器、浏览器、设置、任务管理器和记事本。可以添加本地程序、文档或其他文件，也可以移除任意快捷按钮。"
          error={draft.errors.launcher}
          tag={<UnsupportedTag draft={draft} keyName="launcher" />}
          rowKey="launcher"
        >
          <RecordEditor
            items={readRecords(draft, "launcher")}
            fields={LAUNCHER_FIELDS}
            newItem={LAUNCHER_NEW_ITEM}
            addLabel="添加应用或文件"
            emptyLabel="还没有添加应用或文件。"
            onChange={(next) => draft.setValue("launcher", next)}
          />
        </Row>
        <Note>
          也可以在灵动岛主页点 + 选择文件，或右键移除快捷按钮。
        </Note>
      </Section>

      <PageResetBar onReset={() => draft.resetPage(PAGE_FIELDS[PAGE])} />
    </>
  );
}
