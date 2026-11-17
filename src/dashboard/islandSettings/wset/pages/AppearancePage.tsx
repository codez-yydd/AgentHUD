/**
 * 页①「外观与交互」：位置上贴哪条边、主题与效果、展开与收起、收起态、板块。
 */
import { useEffect, useMemo, useState } from "react";
import {
  Chips,
  FontPicker,
  PageHeader,
  PageResetBar,
  Row,
  Section,
  Select,
  Slider,
  Switch,
} from "../controls";
import {
  HIDDEN_TAB_KEYS,
  HIDDEN_TAB_OPTIONS,
  PAGE_FIELDS,
  type PageKey,
} from "../schema";
import {
  getFonts,
  getMonitors,
  type SettingsFont,
  type SettingsMonitor,
} from "../settingsApi";
import { UnsupportedTag, type PageProps } from "./common";

/** 主题选项：值与文案一一对应 */
const THEME_OPTIONS = [
  { value: "auto", label: "跟随背景自动" },
  { value: "light", label: "浅色毛玻璃" },
  { value: "dark", label: "深色毛玻璃" },
  { value: "rouge", label: "胭脂撞色" },
  { value: "sky", label: "晴空渐变" },
];

const EDGE_OPTIONS = [
  { value: "top", label: "顶部" },
  { value: "bottom", label: "底部" },
  { value: "left", label: "左侧" },
  { value: "right", label: "右侧" },
];

const EXPAND_OPTIONS = [
  { value: "hover", label: "悬停展开" },
  { value: "always", label: "常驻展开" },
  { value: "never", label: "长关" },
];

const COMPACT_OPTIONS = [
  { value: "rings", label: "额度环" },
  { value: "clock", label: "时间天气" },
  { value: "pet", label: "小形象" },
  { value: "music", label: "正在播放" },
];

const QUOTA_DISPLAY_OPTIONS = [
  { value: "rings", label: "纯额度环" },
  { value: "percent", label: "百分比" },
] as const;

// 用现有 Select 组件（其选项不支持逐项 desc），两种配色说明统一放在 Row 的 desc。
const QUOTA_RING_STYLE_OPTIONS = [
  { value: "period", label: "周期分色" },
  { value: "dynamic", label: "用量动态色" },
];

const PAGE: PageKey = "look";

export function AppearancePage({ draft }: PageProps) {
  const [monitors, setMonitors] = useState<SettingsMonitor[]>([]);
  const [fonts, setFonts] = useState<SettingsFont[]>([]);
  const [fontsLoading, setFontsLoading] = useState(true);

  // 显示器 / 本机字体：只在本页挂载时各取一次（低频、失败静默降级）
  useEffect(() => {
    let alive = true;
    getMonitors()
      .then((list) => {
        if (alive) setMonitors(list);
      })
      .catch(() => {
        /* 拿不到显示器列表时退回「显示器 1」，不影响其它项 */
      });
    getFonts()
      .then((list) => {
        if (alive) setFonts(list);
      })
      .catch(() => {
        if (alive) setFonts([]);
      })
      .finally(() => {
        if (alive) setFontsLoading(false);
      });
    return () => {
      alive = false;
    };
  }, []);

  const text = (key: string, fallback = "") => {
    const value = draft.value(key);
    return typeof value === "string" ? value : fallback;
  };
  const num = (key: string, fallback: number) => {
    const value = Number(draft.value(key));
    return Number.isFinite(value) ? value : fallback;
  };
  const flag = (key: string) => Boolean(draft.value(key));

  const themeRaw = text("island.theme", "auto");
  const theme = THEME_OPTIONS.some((item) => item.value === themeRaw) ? themeRaw : "auto";
  const edgeRaw = text("island.edge", "top");
  const edge = EDGE_OPTIONS.some((item) => item.value === edgeRaw) ? edgeRaw : "top";
  const expandRaw = text("island.expand_mode", "hover");
  const expandMode = EXPAND_OPTIONS.some((item) => item.value === expandRaw)
    ? expandRaw
    : "hover";
  const compactRaw = text("island.compact_view", "rings");
  const compactView = COMPACT_OPTIONS.some((item) => item.value === compactRaw)
    ? compactRaw
    : "rings";
  const quotaDisplay = text("island.quota_display") === "rings" ? "rings" : "percent";
  const quotaRingStyleRaw = text("island.quota_ring_style", "period");
  const quotaRingStyle = QUOTA_RING_STYLE_OPTIONS.some((item) => item.value === quotaRingStyleRaw)
    ? quotaRingStyleRaw
    : "period";

  const screen = num("island.screen", 0);
  const monitorOptions = useMemo(() => {
    const options = monitors.map((monitor) => ({
      value: monitor.index,
      label: `${monitor.name}${monitor.primary ? "（主屏）" : ""}`,
    }));
    if (!options.some((item) => item.value === screen)) {
      options.push({ value: screen, label: `显示器 ${screen + 1}` });
    }
    return options;
  }, [monitors, screen]);

  // 沿边位置：配置存 0..1，控件显示 0..100（%）
  const posPct = Math.round(num("island.pos", 0.5) * 100);

  const hiddenValue = draft.value("island.hidden_tabs");
  const hidden = Array.isArray(hiddenValue)
    ? hiddenValue.filter((item): item is string => typeof item === "string")
    : [];
  const shownTabs = HIDDEN_TAB_KEYS.filter((key) => !hidden.includes(key));

  return (
    <>
      <PageHeader
        title="外观与交互"
        desc="位置上贴哪条边、展开方式、主题与收起态内容。拖动灵动岛也会自动记下位置。"
      />

      <Section title="位置与屏幕">
        <Row
          title="吸附位置"
          desc="灵动岛贴在屏幕的哪一条边；贴左 / 右时收起态会自动改成竖排。也可以直接拖它。"
          tag={<UnsupportedTag draft={draft} keyName="island.edge" />}
          rowKey="island.edge"
        >
          <Select
            value={edge}
            width={150}
            options={EDGE_OPTIONS}
            ariaLabel="吸附位置"
            onChange={(next) => draft.setValue("island.edge", next)}
          />
        </Row>
        <Row
          title="沿边位置"
          desc="0% = 最左（上），100% = 最右（下）。拖动灵动岛会自动保存。"
          error={draft.errors["island.pos"]}
          tag={<UnsupportedTag draft={draft} keyName="island.pos" />}
          rowKey="island.pos"
        >
          <Slider
            value={posPct}
            min={0}
            max={100}
            step={1}
            suffix="%"
            ariaLabel="沿边位置"
            onChange={(next) => draft.setValue("island.pos", Math.round(next) / 100)}
          />
        </Row>
        <Row
          title="显示器"
          desc="多显示器时选择灵动岛所在的那一块。"
          tag={<UnsupportedTag draft={draft} keyName="island.screen" />}
          rowKey="island.screen"
        >
          <Select
            value={screen}
            width={170}
            options={monitorOptions}
            ariaLabel="显示器"
            onChange={(next) => draft.setValue("island.screen", next)}
          />
        </Row>
        <Row
          title="开启保持置顶"
          desc="开启后灵动岛始终显示在其他应用窗口前面，悬停展开时也不会被盖住；关闭后可以被其他窗口盖住。"
          tag={<UnsupportedTag draft={draft} keyName="island.keep_on_top" />}
          rowKey="island.keep_on_top"
        >
          <Switch
            checked={flag("island.keep_on_top")}
            ariaLabel="开启保持置顶"
            onChange={(next) => draft.setValue("island.keep_on_top", next)}
          />
        </Row>
        <Row
          title="全屏时贴到屏幕边缘"
          desc="检测到游戏 / 全屏视频时任务栏被遮住，灵动岛会贴住屏幕物理边缘，不再留下任务栏那么高一截空隙；全屏结束后自动滑回原位。"
          tag={<UnsupportedTag draft={draft} keyName="island.hug_fullscreen" />}
          rowKey="island.hug_fullscreen"
        >
          <Switch
            checked={flag("island.hug_fullscreen")}
            ariaLabel="全屏时贴到屏幕边缘"
            onChange={(next) => draft.setValue("island.hug_fullscreen", next)}
          />
        </Row>
      </Section>

      <Section title="主题与效果">
        <Row
          title="主题"
          desc="跟随背景自动切换，或固定一套配色。"
          tag={<UnsupportedTag draft={draft} keyName="island.theme" />}
          rowKey="island.theme"
        >
          <Select
            value={theme}
            width={180}
            options={THEME_OPTIONS}
            ariaLabel="主题"
            onChange={(next) => draft.setValue("island.theme", next)}
          />
        </Row>
        <Row
          title="字体"
          desc="读取本机已安装的字体；选「默认字体」恢复内置字体栈（HarmonyOS Sans → MiSans → 微软雅黑 UI）。字体缺中文字形时，中文会自动回退到系统字体。"
          tag={<UnsupportedTag draft={draft} keyName="island.font_family" />}
          rowKey="island.font_family"
        >
          <FontPicker
            value={text("island.font_family")}
            fonts={fonts}
            loading={fontsLoading}
            onChange={(next) => draft.setValue("island.font_family", next)}
          />
        </Row>
        <Row
          title="出现在截图和录屏里"
          desc="开启时，玻璃直接透出后方画面。关闭时，Windows 可采样窗口正下方的画面用于边缘折射。"
          tag={<UnsupportedTag draft={draft} keyName="island.exclude_from_capture" />}
          rowKey="island.exclude_from_capture"
        >
          {/* 反向语义：控件「开」= exclude_from_capture 为 false */}
          <Switch
            checked={!flag("island.exclude_from_capture")}
            ariaLabel="出现在截图和录屏里"
            onChange={(next) => draft.setValue("island.exclude_from_capture", !next)}
          />
        </Row>
        <Row
          title="空闲呼吸动画"
          desc="收起且静止时，外壳做轻微的缩放呼吸。关闭后灵动岛保持静止，不影响圆角裁剪与内容显示。"
          rowKey="island.breath_enabled"
        >
          <Switch
            checked={flag("island.breath_enabled")}
            ariaLabel="空闲呼吸动画"
            onChange={(next) => draft.setValue("island.breath_enabled", next)}
          />
        </Row>
        <Row
          title="背景刷新率"
          desc="液态玻璃采样桌面画面的频率。频率越高，背景变化跟得越快。"
          error={draft.errors["island.blur_fps"]}
          tag={<UnsupportedTag draft={draft} keyName="island.blur_fps" />}
          rowKey="island.blur_fps"
        >
          <Slider
            value={num("island.blur_fps", 12)}
            min={2}
            max={30}
            step={1}
            suffix="帧/秒"
            ariaLabel="背景刷新率"
            onChange={(next) => draft.setValue("island.blur_fps", Math.round(next))}
          />
        </Row>
        <Row
          title="玻璃透明度"
          desc="调节液态玻璃的透光程度。数值越高，越能透出身后的画面。"
          error={draft.errors["island.transparency"]}
          tag={<UnsupportedTag draft={draft} keyName="island.transparency" />}
          rowKey="island.transparency"
        >
          <Slider
            value={num("island.transparency", 0)}
            min={0}
            max={90}
            step={1}
            suffix="%"
            ariaLabel="玻璃透明度"
            onChange={(next) => draft.setValue("island.transparency", Math.round(next))}
          />
        </Row>
        <Row
          title="界面字号"
          desc="灵动岛界面文字的基础字号；收起态、展开面板与设置窗的预览同步缩放。"
          error={draft.errors["island.font_size"]}
          tag={<UnsupportedTag draft={draft} keyName="island.font_size" />}
          rowKey="island.font_size"
        >
          <Slider
            value={num("island.font_size", 13)}
            min={10}
            max={18}
            step={0.5}
            suffix="px"
            ariaLabel="界面字号"
            onChange={(next) => draft.setValue("island.font_size", next)}
          />
        </Row>
      </Section>

      <Section title="展开与收起">
        <Row
          title="展开方式"
          desc="悬停 = 鼠标移上去展开（关闭「悬停默认展开」后改为单击展开）；常驻 = 一直展开；长关 = 悬停 / 单击都不展开，双击灵动岛可以切回来。"
          tag={<UnsupportedTag draft={draft} keyName="island.expand_mode" />}
          rowKey="island.expand_mode"
        >
          <Select
            value={expandMode}
            width={160}
            options={EXPAND_OPTIONS}
            ariaLabel="展开方式"
            onChange={(next) => draft.setValue("island.expand_mode", next)}
          />
        </Row>
        <Row
          title="悬停默认展开"
          desc="开启后鼠标移到灵动岛就自动展开；关闭后单击灵动岛展开，长按或拖拽不算点击。"
          rowKey="island.hover_expand"
        >
          <Switch
            checked={flag("island.hover_expand")}
            ariaLabel="悬停默认展开"
            onChange={(next) => draft.setValue("island.hover_expand", next)}
          />
        </Row>
        <Row
          title="悬停多少毫秒后展开"
          desc="仅在开启「悬停默认展开」时生效，防止鼠标路过时误展开。"
          error={draft.errors["island.hover_expand_ms"]}
          tag={<UnsupportedTag draft={draft} keyName="island.hover_expand_ms" />}
          rowKey="island.hover_expand_ms"
        >
          <Slider
            value={num("island.hover_expand_ms", 180)}
            min={0}
            max={1000}
            step={10}
            suffix="毫秒"
            ariaLabel="悬停多少毫秒后展开"
            onChange={(next) => draft.setValue("island.hover_expand_ms", Math.round(next))}
          />
        </Row>
        <Row
          title="移开后多少毫秒收起"
          desc="鼠标离开后等待的时间。"
          error={draft.errors["island.collapse_delay_ms"]}
          tag={<UnsupportedTag draft={draft} keyName="island.collapse_delay_ms" />}
          rowKey="island.collapse_delay_ms"
        >
          <Slider
            value={num("island.collapse_delay_ms", 260)}
            min={0}
            max={2000}
            step={20}
            suffix="毫秒"
            ariaLabel="移开后多少毫秒收起"
            onChange={(next) => draft.setValue("island.collapse_delay_ms", Math.round(next))}
          />
        </Row>
        <Row
          title="鼠标穿透"
          desc="鼠标直接穿过灵动岛，只响应右键。"
          tag={<UnsupportedTag draft={draft} keyName="island.passthrough" />}
          rowKey="island.passthrough"
        >
          <Switch
            checked={flag("island.passthrough")}
            ariaLabel="鼠标穿透"
            onChange={(next) => draft.setValue("island.passthrough", next)}
          />
        </Row>
        <Row
          title="前台全屏时隐藏"
          desc="检测到游戏 / 全屏视频时自动收起并隐藏。"
          tag={<UnsupportedTag draft={draft} keyName="island.hide_fullscreen" />}
          rowKey="island.hide_fullscreen"
        >
          <Switch
            checked={flag("island.hide_fullscreen")}
            ariaLabel="前台全屏时隐藏"
            onChange={(next) => draft.setValue("island.hide_fullscreen", next)}
          />
        </Row>
      </Section>

      <Section title="收起态">
        <Row
          title="默认内容"
          desc="滚轮可以在几种视图之间切换。"
          tag={<UnsupportedTag draft={draft} keyName="island.compact_view" />}
          rowKey="island.compact_view"
        >
          <Select
            value={compactView}
            width={160}
            options={COMPACT_OPTIONS}
            ariaLabel="默认内容"
            onChange={(next) => draft.setValue("island.compact_view", next)}
          />
        </Row>
        <Row
          title="额度环显示方式"
          error={draft.errors["island.quota_display"]}
          tag={<UnsupportedTag draft={draft} keyName="island.quota_display" />}
          rowKey="island.quota_display"
        >
          <div
            className="wset-quota-display"
            role="radiogroup"
            aria-label="额度环显示方式"
            data-view={quotaDisplay}
          >
            {QUOTA_DISPLAY_OPTIONS.map(({ value, label }) => (
              <label className="wset-quota-display__option" key={value}>
                <input
                  type="radio"
                  name="island.quota_display"
                  value={value}
                  checked={quotaDisplay === value}
                  onChange={() => draft.setValue("island.quota_display", value)}
                />
                <span>{label}</span>
              </label>
            ))}
          </div>
        </Row>
        {quotaDisplay === "rings" ? (
          <Row
            title="额度环配色"
            desc="周期分色：5h / 周 / 月各自固定颜色；用量动态色：按已用额度绿黄红渐变提示剩余量。"
            error={draft.errors["island.quota_ring_style"]}
            tag={<UnsupportedTag draft={draft} keyName="island.quota_ring_style" />}
            rowKey="island.quota_ring_style"
          >
            <Select
              value={quotaRingStyle}
              width={160}
              options={QUOTA_RING_STYLE_OPTIONS}
              ariaLabel="额度环配色"
              onChange={(next) => draft.setValue("island.quota_ring_style", next)}
            />
          </Row>
        ) : null}
        <Row
          title="自动轮换间隔"
          desc="时间天气与小形象每隔多久自动切换一次；0 = 不轮换。"
          error={draft.errors["island.auto_cycle_s"]}
          tag={<UnsupportedTag draft={draft} keyName="island.auto_cycle_s" />}
          rowKey="island.auto_cycle_s"
        >
          <Slider
            value={num("island.auto_cycle_s", 0)}
            min={0}
            max={600}
            step={10}
            suffix="秒"
            ariaLabel="自动轮换间隔"
            onChange={(next) => draft.setValue("island.auto_cycle_s", Math.round(next))}
          />
        </Row>
      </Section>

      <Section title="板块">
        <Row
          wide
          title="显示的板块"
          desc="点击板块名称可在标签栏显示或隐藏。隐藏后会立即停止对应后台工作：股票=行情轮询、消息=通知轮询与浮窗扫描、截图=销毁截图/录屏预热窗口、控制=系统状态轮询、剪贴板=剪贴板监听、工具=停止正在进行的会议录音（已录内容正常保存，不保留后台录音）；天气与媒体因主页/收起态仍展示，保持轮询。"
          tag={<UnsupportedTag draft={draft} keyName="island.hidden_tabs" />}
          rowKey="island.hidden_tabs"
        >
          <Chips
            options={HIDDEN_TAB_OPTIONS}
            value={shownTabs}
            ariaLabel="显示的板块"
            // 配置存的是 hidden_tabs：控件里「已显示」的集合取反后写回；
            // 选项之外的键（后端也接受 msg）原样保留，避免一次编辑把它们清掉
            onChange={(next) =>
              draft.setValue("island.hidden_tabs", [
                ...HIDDEN_TAB_KEYS.filter((key) => !next.includes(key)),
                ...hidden.filter((key) => !HIDDEN_TAB_KEYS.includes(key)),
              ])
            }
          />
        </Row>
      </Section>

      <PageResetBar onReset={() => draft.resetPage(PAGE_FIELDS[PAGE])} />
    </>
  );
}
