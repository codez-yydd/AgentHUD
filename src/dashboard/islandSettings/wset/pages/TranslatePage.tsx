/**
 * 页⑨「翻译」：翻译来源（已配置平台 / 自定义接口）与测试。
 */
import { useEffect, useRef, useState } from "react";
import {
  Button,
  PageHeader,
  PageResetBar,
  Row,
  Section,
  Switch,
  TextInput,
} from "../controls";
import { PAGE_FIELDS, type PageKey } from "../schema";
import {
  listTranslationModels,
  listTranslationPlatforms,
  translateText,
  type TranslationPlatform,
} from "../settingsApi";
import {
  errorText,
  readFlag,
  readText,
  UnsupportedTag,
  type PageProps,
} from "./common";

const PAGE: PageKey = "translate";
/// focus 自动刷新的最小间隔：窗口在短时间内反复获得焦点时只刷一次。
const FOCUS_REFRESH_MIN_MS = 800;
/// 已保存平台失效且没有可用选项时，select 里用于清空草稿的哨兵值。
const CLEAR_PLATFORM_VALUE = "__clear__";

function TranslationSelect({
  value,
  options,
  placeholder,
  missingLabel,
  label,
  loading,
  onChange,
  onClear,
}: {
  value: string;
  options: TranslationPlatform[];
  placeholder: string;
  missingLabel: string;
  label: string;
  loading: boolean;
  onChange: (next: string) => void;
  onClear?: () => void;
}) {
  const missing = value !== "" && !options.some(({ id }) => id === value);
  const canClear = missing && Boolean(onClear);
  return (
    <select
      className="wset-select"
      style={{ width: 250 }}
      value={value}
      aria-label={label}
      aria-busy={loading || undefined}
      disabled={!options.length && !canClear}
      onChange={(event) => {
        const next = event.currentTarget.value;
        if (next === CLEAR_PLATFORM_VALUE) {
          onClear?.();
          return;
        }
        onChange(next);
      }}
    >
      <option value="" disabled>{placeholder}</option>
      {missing ? <option value={value} disabled>{missingLabel}</option> : null}
      {canClear ? <option value={CLEAR_PLATFORM_VALUE}>清除已保存的平台（置空）</option> : null}
      {options.map(({ id, label: optionLabel }) => (
        <option key={id} value={id}>{optionLabel}</option>
      ))}
    </select>
  );
}

function PlatformTranslationFields({ draft }: Pick<PageProps, "draft">) {
  const [refreshCount, setRefreshCount] = useState(0);
  const [platformResult, setPlatformResult] = useState<{
    revision: number;
    items: TranslationPlatform[];
    error: string | null;
  } | null>(null);
  const [modelResult, setModelResult] = useState<{
    revision: number;
    platform: string;
    items: string[];
    error: string | null;
  } | null>(null);
  const lastRefreshAt = useRef(Date.now());
  const platform = readText(draft, "translate.platform");
  const model = readText(draft, "translate.platform_model");
  // 刷新期间保留上一版 options（stale-while-revalidate），只标记 loading；
  // revision 只用于判断结果是否属于本轮请求，不再清空列表。
  const platformLoading = !platformResult || platformResult.revision !== refreshCount;
  const platforms = platformResult?.items ?? [];
  const platformError = platformResult?.error ?? null;
  const platformAvailable = platforms.some(({ id }) => id === platform);
  const modelLoading = platformAvailable
    && (modelResult?.platform !== platform || modelResult.revision !== refreshCount);
  const models = platformAvailable && modelResult?.platform === platform ? modelResult.items : [];
  const modelError = platformAvailable && modelResult?.platform === platform
    ? modelResult.error
    : null;

  useEffect(() => {
    let active = true;
    const revision = refreshCount;
    void listTranslationPlatforms()
      .then((items) => {
        if (active) setPlatformResult({ revision, items, error: null });
      })
      .catch((error) => {
        if (!active) return;
        setPlatformResult((previous) => ({
          revision,
          items: previous?.items ?? [],
          error: errorText(error),
        }));
      });
    return () => { active = false; };
  }, [refreshCount]);

  // 设置窗重新获得焦点时重拉平台列表：主界面新加凭证 / 刚完成 CLI 登录后
  // 回到本窗即可看到，无需手动点刷新。800ms 内的连续 focus 只刷一次；
  // 版本号仍用于丢弃过期结果。
  useEffect(() => {
    const reload = () => {
      const now = Date.now();
      if (now - lastRefreshAt.current < FOCUS_REFRESH_MIN_MS) return;
      lastRefreshAt.current = now;
      setRefreshCount((count) => count + 1);
    };
    window.addEventListener("focus", reload);
    return () => window.removeEventListener("focus", reload);
  }, []);

  useEffect(() => {
    if (!platformAvailable) return;
    let active = true;
    const revision = refreshCount;
    const target = platform;
    void listTranslationModels(target)
      .then((items) => {
        if (active) setModelResult({ revision, platform: target, items, error: null });
      })
      .catch((error) => {
        if (!active) return;
        setModelResult((previous) => ({
          revision,
          platform: target,
          items: previous?.platform === target ? previous.items : [],
          error: errorText(error),
        }));
      });
    return () => { active = false; };
  }, [platform, platformAvailable, refreshCount]);

  const platformDescription = platformLoading
    ? "正在刷新可用平台…"
    : platformError
      ? `平台列表读取失败：${platformError}。凭证/登录态失效需到主界面重新配置或重新登录；其他原因可点「刷新」重试。`
      : platforms.length === 0
        ? "未找到可用于翻译的平台：请先在主界面为 DeepSeek / Kimi / Moonshot / MiniMax / 阿里云百炼 / Gemini / Grok / 豆包 / OpenCode Go 添加 API Key，或先完成 Kimi / MiniMax / OpenCode 的 CLI 登录，再点「刷新」。"
        : platform && !platformAvailable
          ? "已保存的平台已不可用（凭证可能已删除，或本机登录态已失效），请重新选择或清除已保存的平台。"
          : "从下面选择用于翻译的平台；来源包括主界面已保存的凭证与本机 CLI 登录态。";
  const modelDescription = !platformAvailable
    ? undefined
    : modelLoading
      ? "正在刷新模型列表…"
      : modelError
        ? `模型列表读取失败：${modelError}`
        : models.length === 0
          ? "该平台暂无可选模型。"
          : model && !models.includes(model)
            ? "已保存的模型未出现在当前列表中。"
            : undefined;

  return (
    <>
      <Row
        title="翻译平台"
        desc={platformDescription}
        tag={<UnsupportedTag draft={draft} keyName="translate.platform" />}
        rowKey="translate.platform"
      >
        <TranslationSelect
          value={platform}
          options={platforms}
          placeholder={platformLoading ? "正在加载平台…" : "选择平台"}
          missingLabel={platformLoading ? "正在加载平台…" : "已保存的平台不可用"}
          label="翻译平台"
          loading={platformLoading}
          onChange={(next) => draft.setMany({ "translate.platform": next, "translate.platform_model": "" })}
          onClear={() => draft.setMany({ "translate.platform": "", "translate.platform_model": "" })}
        />
        <Button
          size="sm"
          ariaLabel="刷新平台和模型"
          onClick={() => {
            lastRefreshAt.current = Date.now();
            setRefreshCount((count) => count + 1);
          }}
        >
          刷新
        </Button>
      </Row>
      <Row
        title="模型"
        desc={modelDescription}
        tag={<UnsupportedTag draft={draft} keyName="translate.platform_model" />}
        rowKey="translate.platform_model"
      >
        <TranslationSelect
          value={model}
          options={models.map((id) => ({ id, label: id }))}
          placeholder={!platformAvailable ? "请先选择平台" : modelLoading ? "正在加载模型…" : "选择模型"}
          missingLabel={model}
          label="翻译模型"
          loading={modelLoading}
          onChange={(next) => draft.setValue("translate.platform_model", next)}
        />
      </Row>
    </>
  );
}

function CustomTranslationFields({ draft }: Pick<PageProps, "draft">) {
  return (
    <>
      <Row
        title="API Key"
        tag={<UnsupportedTag draft={draft} keyName="translate.api_key" />}
        rowKey="translate.api_key"
      >
        <TextInput
          value={readText(draft, "translate.api_key")}
          width={250}
          secret
          ariaLabel="自定义 API Key"
          onChange={(next) => draft.setValue("translate.api_key", next)}
        />
      </Row>
      <Row
        title="接口地址"
        desc="填写 OpenAI 兼容接口地址。"
        error={draft.errors["translate.base_url"]}
        tag={<UnsupportedTag draft={draft} keyName="translate.base_url" />}
        rowKey="translate.base_url"
      >
        <TextInput
          value={readText(draft, "translate.base_url")}
          width={250}
          invalid={Boolean(draft.errors["translate.base_url"])}
          ariaLabel="自定义接口地址"
          onChange={(next) => draft.setValue("translate.base_url", next)}
        />
      </Row>
      <Row
        title="模型"
        desc="填写接口使用的模型 ID。"
        tag={<UnsupportedTag draft={draft} keyName="translate.model" />}
        rowKey="translate.model"
      >
        <TextInput
          value={readText(draft, "translate.model")}
          width={250}
          ariaLabel="自定义模型"
          onChange={(next) => draft.setValue("translate.model", next)}
        />
      </Row>
    </>
  );
}

export function TranslatePage({ draft }: PageProps) {
  const [translating, setTranslating] = useState(false);
  const customEnabled = readFlag(draft, "translate.custom_enabled");

  /** 「测试翻译」：用固定示例句子调一次接口，不碰草稿、不触发保存 */
  const runTranslateTest = () => {
    if (translating) return;
    setTranslating(true);
    void translateText("Hello, world!")
      .then((translation) => draft.setStatus(`翻译结果：${translation}`, "ok", true))
      .catch((error) => draft.setStatus(`翻译失败：${errorText(error)}`, "danger", true))
      .finally(() => setTranslating(false));
  };

  return (
    <>
      <PageHeader
        title="翻译"
        desc="选择翻译平台和模型，或用自定义接口。"
      />

      <Section title="翻译">
        <Row
          title="自定义接口"
          desc="开启后使用自定义 API Key、接口地址和模型。"
          tag={<UnsupportedTag draft={draft} keyName="translate.custom_enabled" />}
          rowKey="translate.custom_enabled"
        >
          <Switch
            checked={customEnabled}
            ariaLabel="自定义接口"
            onChange={(next) => draft.setValue("translate.custom_enabled", next)}
          />
        </Row>
        {customEnabled ? <CustomTranslationFields draft={draft} /> : <PlatformTranslationFields draft={draft} />}
        <Row
          title="测试翻译"
          desc="使用已保存的配置翻译示例句子「Hello, world!」。"
        >
          <Button onClick={runTranslateTest} disabled={translating}>
            {translating ? "翻译中…" : "测试翻译"}
          </Button>
        </Row>
      </Section>

      <PageResetBar onReset={() => draft.resetPage(PAGE_FIELDS[PAGE])} />
    </>
  );
}
