/**
 * 页⑩「实时字幕」：运行状态（模型下载 / 开始聆听）+ 声音来源（VAD 断句）
 * → 日语识别（ASR / 草稿刷新 / 线程）→ 字幕翻译。
 * 16 行、Section 划分与数值范围 / 步进 / 小数位集中在本页定义；
 * 顶部「运行状态」是本页新增的运行时区块（命令 live_* + 事件 zbar://island-live）。
 */
import { useEffect, useState } from "react";
import {
  Button,
  Note,
  PageHeader,
  PageResetBar,
  Row,
  Section,
  Select,
  StatusDot,
  Stepper,
  Switch,
  TextInput,
} from "../controls";
import { PAGE_FIELDS, type PageKey } from "../schema";
import {
  browsePath,
  errorText,
  readFlag,
  readNumber,
  readText,
  type PageProps,
} from "./common";
import {
  LIVE_MODEL_TOTAL_FALLBACK,
  liveCancelDownload,
  liveDevices,
  liveDownloadModels,
  liveStart,
  liveStatus,
  liveStop,
  onLiveStatus,
} from "../../../../island/api";
import type { LiveDevices, LiveStatus } from "../../../../island/types";

const PAGE: PageKey = "live";

const SOURCE_OPTIONS = [
  { value: "loopback", label: "系统声音" },
  { value: "mic", label: "麦克风" },
];

/** 字节 → MB（整数；进度显示 402/630 这种粒度即可） */
function fmtMb(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0";
  return String(Math.round(bytes / (1024 * 1024)));
}

/** 下载总大小：后端拿不到时按 630MB 估算，避免百分比恒为 0 */
function downloadTotal(status: LiveStatus | null): number {
  const total = status?.download.total ?? 0;
  return total > 0 ? total : LIVE_MODEL_TOTAL_FALLBACK;
}

/** 运行阶段 → 中文（idle / starting / listening / stopping / error） */
function phaseLabel(status: LiveStatus | null): string {
  if (!status) return "未开始";
  switch (status.phase) {
    case "starting":
      return "正在启动…";
    case "listening":
      return "聆听中";
    case "stopping":
      return "正在停止…";
    case "error":
      return status.error || "启动失败";
    default:
      return status.running ? "聆听中" : "未开始";
  }
}

/** 进度条（设置窗没有通用进度条组件，这里用主题变量内联绘制） */
function ProgressBar({ percent }: { percent: number }) {
  return (
    <span
      aria-hidden="true"
      style={{
        display: "block",
        width: 220,
        height: 6,
        borderRadius: 3,
        background: "var(--w-track)",
        overflow: "hidden",
      }}
    >
      <i
        style={{
          display: "block",
          height: "100%",
          width: `${percent}%`,
          borderRadius: 3,
          background: "var(--w-on)",
          transition: "width 200ms ease",
        }}
      />
    </span>
  );
}

export function LivePage({ draft }: PageProps) {
  // ===== 运行时状态：命令一次拉取 + 事件持续刷新（下载进度也走这个事件） =====
  const [status, setStatus] = useState<LiveStatus | null>(null);
  const [devices, setDevices] = useState<LiveDevices | null>(null);
  const [busy, setBusy] = useState(false);
  const [runtimeError, setRuntimeError] = useState("");

  useEffect(() => {
    let alive = true;
    void liveStatus()
      .then((next) => {
        if (alive) setStatus(next);
      })
      .catch((error) => {
        // 后端（并行开发中）还没注册命令时静默降级：不挡住本页设置项。
        if (alive) setRuntimeError(errorText(error));
      });
    void liveDevices()
      .then((next) => {
        if (alive) setDevices(next);
      })
      .catch(() => {});
    const off = onLiveStatus((next) => {
      if (alive && next) setStatus(next);
    });
    return () => {
      alive = false;
      off();
    };
  }, []);

  /** 运行类命令统一收口：busy 防连点、返回值写回状态、失败只在行内提示 */
  const run = (action: () => Promise<LiveStatus>) => {
    if (busy) return;
    setBusy(true);
    setRuntimeError("");
    void action()
      .then((next) => {
        if (next) setStatus(next);
      })
      .catch((error) => setRuntimeError(errorText(error)))
      .finally(() => setBusy(false));
  };

  const running = Boolean(status?.running);
  const modelReady = Boolean(status?.modelReady);
  const download = status?.download;
  const downloading = download?.state === "downloading" || download?.state === "verifying";
  const verifying = download?.state === "verifying";
  const totalBytes = downloadTotal(status);
  const percent = downloading
    ? Math.max(0, Math.min(100, Math.round(((download?.done ?? 0) / totalBytes) * 100)))
    : download?.state === "done"
      ? 100
      : 0;
  const downloadDesc = downloading
    ? `${verifying ? "正在校验" : "正在下载"} ${percent}%（${fmtMb(download?.done ?? 0)}/${fmtMb(totalBytes)} MB）${
        download?.label ? ` · ${download.label}` : ""
      }`
    : download?.state === "error"
      ? `下载失败：${download.message || "未知原因"}`
      : modelReady
        ? "模型已下载，可以直接开始聆听。"
        : `未下载（约 ${fmtMb(LIVE_MODEL_TOTAL_FALLBACK)}MB），首次使用前需要下载。`;

  // 设备下拉：按当前采集来源取 loopback / mic，第一项「默认设备」= 空串
  const source = readText(draft, "live.audio.source", "loopback");
  const deviceNames = (source === "mic" ? devices?.mic : devices?.loopback) ?? [];
  const deviceOptions = [
    { value: "", label: "默认设备" },
    ...deviceNames.map((name) => ({ value: name, label: name })),
  ];
  const deviceValue = readText(draft, "live.audio.device");
  // 草稿里留着的设备名可能已经拔掉：仍保留为一项，避免保存时被悄悄清空
  const deviceOptionsWithCurrent =
    deviceValue && !deviceNames.includes(deviceValue)
      ? [...deviceOptions, { value: deviceValue, label: `${deviceValue}（未连接）` }]
      : deviceOptions;

  return (
    <>
      <PageHeader
        title="实时字幕"
        desc="系统声音 → 日语识别 → 中文翻译的实时字幕参数。翻译使用「翻译」页配置的数据源；修改采集 / VAD / ASR 参数后需重新开始聆听生效。"
      />

      <Section title="运行状态">
        <Row
          title="识别模型"
          desc={downloadDesc}
          error={download?.state === "error" ? download.message || "模型下载失败" : undefined}
        >
          <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
            {downloading ? <ProgressBar percent={percent} /> : null}
            <span
              style={{
                fontSize: 12.5,
                color: modelReady ? "var(--w-ok)" : "var(--w-text2)",
                whiteSpace: "nowrap",
              }}
            >
              {modelReady ? "已下载" : downloading ? `${percent}%` : "未下载"}
            </span>
            {downloading ? (
              <Button
                size="sm"
                onClick={() => run(liveCancelDownload)}
                disabled={busy}
                title="取消模型下载"
              >
                取消下载
              </Button>
            ) : (
              <Button
                size="sm"
                variant="primary"
                onClick={() => run(liveDownloadModels)}
                disabled={busy || modelReady}
                title="下载 parakeet-ja 与 silero_vad（约 630MB）"
              >
                {modelReady ? "已下载" : "下载模型"}
              </Button>
            )}
          </div>
        </Row>
        <Row
          title="聆听状态"
          desc="开始后采集系统声音（或麦克风）→ VAD 断句 → 日语识别 → 中文翻译；收起胶囊上显示最新一句。"
        >
          <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
            <StatusDot
              role={running ? "ok" : status?.phase === "error" ? "danger" : "hint"}
            />
            <span
              style={{
                fontSize: 12.5,
                color: status?.phase === "error" ? "var(--w-danger)" : "var(--w-text2)",
                maxWidth: 260,
                overflow: "hidden",
                textOverflow: "ellipsis",
                whiteSpace: "nowrap",
              }}
              title={phaseLabel(status)}
            >
              {phaseLabel(status)}
            </span>
            <Button
              size="sm"
              variant={running ? "ghost" : "primary"}
              onClick={() => run(running ? liveStop : liveStart)}
              disabled={busy}
            >
              {running ? "停止" : "开始聆听"}
            </Button>
          </div>
        </Row>
        {runtimeError ? (
          <Note>
            运行状态读取失败（后端可能还没就绪）：{runtimeError}
          </Note>
        ) : null}
      </Section>

      <Section title="声音来源">
        <Row
          title="采集来源"
          desc="系统声音 = 采集扬声器输出；麦克风 = 采集环境声。"
          error={draft.errors["live.audio.source"]}
          rowKey="live.audio.source"
        >
          <Select
            value={readText(draft, "live.audio.source", "loopback")}
            width={150}
            options={SOURCE_OPTIONS}
            ariaLabel="采集来源"
            onChange={(next) => draft.setValue("live.audio.source", next)}
          />
        </Row>
        <Row
          title="设备名"
          desc="留空使用默认设备；下拉里是当前采集来源的可用设备。"
          rowKey="live.audio.device"
        >
          <Select
            value={deviceValue}
            width={280}
            options={deviceOptionsWithCurrent}
            ariaLabel="音频设备"
            onChange={(next) => draft.setValue("live.audio.device", next)}
          />
        </Row>
        <Row
          title="静音检测阈值"
          desc="0.1 ~ 0.9，越大越难把声音判定为说话。"
          error={draft.errors["live.vad.threshold"]}
          rowKey="live.vad.threshold"
        >
          <Stepper
            value={readNumber(draft, "live.vad.threshold", 0.4)}
            min={0.1}
            max={0.9}
            step={0.05}
            decimals={2}
            ariaLabel="静音检测阈值"
            onChange={(next) => draft.setValue("live.vad.threshold", next)}
          />
        </Row>
        <Row
          title="最短静音"
          desc="静音超过这个长度就断句。"
          error={draft.errors["live.vad.min_silence_ms"]}
          rowKey="live.vad.min_silence_ms"
        >
          <Stepper
            value={readNumber(draft, "live.vad.min_silence_ms", 350)}
            min={100}
            max={1500}
            step={50}
            suffix="毫秒"
            ariaLabel="最短静音"
            onChange={(next) => draft.setValue("live.vad.min_silence_ms", next)}
          />
        </Row>
        <Row
          title="最短语音"
          desc="短于这个长度的声音忽略。"
          error={draft.errors["live.vad.min_speech_ms"]}
          rowKey="live.vad.min_speech_ms"
        >
          <Stepper
            value={readNumber(draft, "live.vad.min_speech_ms", 200)}
            min={100}
            max={1000}
            step={50}
            suffix="毫秒"
            ariaLabel="最短语音"
            onChange={(next) => draft.setValue("live.vad.min_speech_ms", next)}
          />
        </Row>
      </Section>

      <Section title="日语识别">
        <Row
          title="模型目录"
          desc="相对路径以 ~/.agenthud/ 为基准（默认 models），也支持绝对路径；模型约 630MB，首次使用前下载。"
          rowKey="live.models_dir"
        >
          <TextInput
            value={readText(draft, "live.models_dir")}
            width={280}
            ariaLabel="模型目录"
            onBrowse={() => void browsePath(draft, "live.models_dir", { directory: true })}
            browseTitle="选择模型目录"
            onChange={(next) => draft.setValue("live.models_dir", next)}
          />
        </Row>
        <Row
          title="实时显示正在识别的内容"
          desc="关闭后只显示整句校对结果。"
          rowKey="live.asr.partial"
        >
          <Switch
            checked={readFlag(draft, "live.asr.partial", true)}
            ariaLabel="实时显示正在识别的内容"
            onChange={(next) => draft.setValue("live.asr.partial", next)}
          />
        </Row>
        <Row
          title="草稿刷新间隔"
          desc="越小出字越快，也越占 CPU。"
          error={draft.errors["live.asr.draft_interval_ms"]}
          rowKey="live.asr.draft_interval_ms"
        >
          <Stepper
            value={readNumber(draft, "live.asr.draft_interval_ms", 500)}
            min={200}
            max={2000}
            step={50}
            suffix="毫秒"
            ariaLabel="草稿刷新间隔"
            onChange={(next) => draft.setValue("live.asr.draft_interval_ms", next)}
          />
        </Row>
        <Row
          title="识别线程数"
          desc="越大越快，也越占 CPU。"
          error={draft.errors["live.asr.final_threads"]}
          rowKey="live.asr.final_threads"
        >
          <Stepper
            value={readNumber(draft, "live.asr.final_threads", 6)}
            min={1}
            max={16}
            ariaLabel="识别线程数"
            onChange={(next) => draft.setValue("live.asr.final_threads", next)}
          />
        </Row>
        <Row
          title="单句最长时长"
          desc="超过就强制断句。"
          error={draft.errors["live.asr.max_segment_s"]}
          rowKey="live.asr.max_segment_s"
        >
          <Stepper
            value={readNumber(draft, "live.asr.max_segment_s", 10)}
            min={3}
            max={30}
            suffix="秒"
            ariaLabel="单句最长时长"
            onChange={(next) => draft.setValue("live.asr.max_segment_s", next)}
          />
        </Row>
        <Row
          title="翻译时带上文"
          desc="仅影响翻译（用于理解指代/语气），会增加翻译 token 消耗；不影响识别准确率。"
          rowKey="live.asr.context_prompt"
        >
          <Switch
            checked={readFlag(draft, "live.asr.context_prompt")}
            ariaLabel="翻译时带上文"
            onChange={(next) => draft.setValue("live.asr.context_prompt", next)}
          />
        </Row>
      </Section>

      <Section title="字幕翻译">
        <Row
          title="翻译成中文"
          desc="关闭后只显示日语识别结果。"
          rowKey="live.translate.enabled"
        >
          <Switch
            checked={readFlag(draft, "live.translate.enabled", true)}
            ariaLabel="翻译成中文"
            onChange={(next) => draft.setValue("live.translate.enabled", next)}
          />
        </Row>
        <Row
          title="边说边译"
          desc="只翻译还在滚动的草稿；关闭后等整句出来再翻。开启后会显著增加翻译调用次数与费用；同一句会随草稿多次翻译。"
          rowKey="live.translate.live"
        >
          <Switch
            checked={readFlag(draft, "live.translate.live", true)}
            ariaLabel="边说边译"
            onChange={(next) => draft.setValue("live.translate.live", next)}
          />
        </Row>
        <Row
          title="最小翻译间隔"
          desc="限制翻译请求频率。"
          error={draft.errors["live.translate.live_min_interval_ms"]}
          rowKey="live.translate.live_min_interval_ms"
        >
          <Stepper
            value={readNumber(draft, "live.translate.live_min_interval_ms", 900)}
            min={300}
            max={3000}
            step={100}
            suffix="毫秒"
            ariaLabel="最小翻译间隔"
            onChange={(next) => draft.setValue("live.translate.live_min_interval_ms", next)}
          />
        </Row>
        <Row
          title="最少新增字数"
          desc="新增不足这么多字就暂不翻译。"
          error={draft.errors["live.translate.live_min_new_chars"]}
          rowKey="live.translate.live_min_new_chars"
        >
          <Stepper
            value={readNumber(draft, "live.translate.live_min_new_chars", 4)}
            min={1}
            max={20}
            ariaLabel="最少新增字数"
            onChange={(next) => draft.setValue("live.translate.live_min_new_chars", next)}
          />
        </Row>
        <Row
          title="上下文行数"
          desc="带上前面几句作为翻译上下文。需开启上一项『翻译时带上文』才生效。"
          error={draft.errors["live.translate.context_lines"]}
          rowKey="live.translate.context_lines"
        >
          <Stepper
            value={readNumber(draft, "live.translate.context_lines", 3)}
            min={0}
            max={3}
            ariaLabel="上下文行数"
            onChange={(next) => draft.setValue("live.translate.context_lines", next)}
          />
        </Row>
      </Section>

      <PageResetBar onReset={() => draft.resetPage(PAGE_FIELDS[PAGE])} />
    </>
  );
}
