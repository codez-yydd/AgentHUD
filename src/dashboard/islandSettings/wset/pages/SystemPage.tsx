/**
 * 页⑫「系统与控制」：Clash Verge 配置目录 / 混合代理端口 / 程序路径
 */
import {
  PageHeader,
  PageResetBar,
  Row,
  Section,
  Stepper,
  TextInput,
} from "../controls";
import { PAGE_FIELDS, type PageKey } from "../schema";
import {
  browsePath,
  readNumber,
  readText,
  UnsupportedTag,
  type PageProps,
} from "./common";

const PAGE: PageKey = "system";

export function SystemPage({ draft }: PageProps) {
  return (
    <>
      <PageHeader
        title="系统与控制"
        desc="Clash Verge 与亮度控制（Twinkle）的本地路径。"
      />

      <Section title="Clash Verge">
        <Row
          title="配置目录"
          desc="从这里的 config.yaml 读取控制器地址与 secret；支持 %APPDATA% 这类环境变量。"
          tag={<UnsupportedTag draft={draft} keyName="clash.config_dir" />}
          rowKey="clash.config_dir"
        >
          <TextInput
            value={readText(draft, "clash.config_dir")}
            width={280}
            placeholder="%APPDATA%/…"
            ariaLabel="Clash Verge 配置目录"
            onBrowse={() => void browsePath(draft, "clash.config_dir", { directory: true })}
            browseTitle="选择配置目录"
            onChange={(next) => draft.setValue("clash.config_dir", next)}
          />
        </Row>
        <Row
          title="混合代理端口"
          desc="读取不到 Clash Verge 的混合端口时，用于识别该应用的系统代理。"
          error={draft.errors["clash.proxy_port"]}
          tag={<UnsupportedTag draft={draft} keyName="clash.proxy_port" />}
          rowKey="clash.proxy_port"
        >
          <Stepper
            value={readNumber(draft, "clash.proxy_port", 7897)}
            min={1}
            max={65535}
            ariaLabel="混合代理端口"
            onChange={(next) => draft.setValue("clash.proxy_port", next)}
          />
        </Row>
        <Row
          title="Clash Verge 程序"
          desc="「打开 Clash Verge」按钮使用；留空时启动自动探测本机安装位置并写入。"
          tag={<UnsupportedTag draft={draft} keyName="clash.app" />}
          rowKey="clash.app"
        >
          <TextInput
            value={readText(draft, "clash.app")}
            width={280}
            ariaLabel="Clash Verge 程序路径"
            onBrowse={() => void browsePath(draft, "clash.app", { title: "选择 Clash Verge" })}
            browseTitle="选择程序"
            onChange={(next) => draft.setValue("clash.app", next)}
          />
        </Row>
      </Section>

      <Section title="亮度">
        <Row
          title="Twinkle 便携版路径"
          desc="留空使用 DDC/CI / WMI；填写 Twinkle 可执行文件后，亮度滑块改走 Twinkle 命令行（--All --Set，拖动时按 120ms 去抖调用、同值 800ms 内去重、串行执行，失败自动回退 DDC/CI；便携版首次调用可能短暂显示「安装中」）。启动时路径为空会自动探测常见位置。"
          tag={<UnsupportedTag draft={draft} keyName="brightness.twinkle" />}
          rowKey="brightness.twinkle"
        >
          <TextInput
            value={readText(draft, "brightness.twinkle")}
            width={280}
            ariaLabel="Twinkle 便携版路径"
            onBrowse={() => void browsePath(draft, "brightness.twinkle", { title: "选择 Twinkle" })}
            browseTitle="选择程序"
            onChange={(next) => draft.setValue("brightness.twinkle", next)}
          />
        </Row>
      </Section>

      <PageResetBar onReset={() => draft.resetPage(PAGE_FIELDS[PAGE])} />
    </>
  );
}
