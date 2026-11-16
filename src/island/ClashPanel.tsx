import { useEffect, useRef, useState } from "react";
import { getClashStatus, openClashVerge, setClashMode, setClashToggle } from "./api";
import type { ClashMode, ClashStatus, ClashToggleKind } from "./types";
import { Icon, Toggle } from "./ui";

const MODES = [
  { value: "rule", label: "规则" },
  { value: "global", label: "全局" },
  { value: "direct", label: "直连" },
] as const satisfies ReadonlyArray<{ value: ClashMode; label: string }>;
const SWITCHES = [
  { kind: "systemProxy", label: "系统代理", capability: "canSetSystemProxy" },
  { kind: "tun", label: "TUN 模式", capability: "canSetTun" },
] as const;

function formatTraffic(bytes: number): string {
  const units = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
  const unit = bytes >= 1 ? Math.min(units.length - 1, Math.floor(Math.log2(bytes) / 10)) : 0;
  const amount = bytes / 1024 ** unit;
  return `${Number(amount.toFixed(unit === 0 ? 0 : amount >= 100 ? 1 : 2))} ${units[unit]}`;
}

function formatSpeed(bytes: number | undefined): string {
  return bytes == null || !Number.isFinite(bytes) || bytes < 0 ? "—" : `${formatTraffic(bytes)}/s`;
}

export function ClashPanel({ active, onConfigure, onError }: {
  active: boolean;
  onConfigure: () => void;
  onError: (error: string) => void;
}) {
  const [status, setStatus] = useState<ClashStatus | null>();
  const [operation, setOperation] = useState<ClashToggleKind | "open" | "mode" | null>(null);
  const actionInFlight = useRef(false);
  const actionVersion = useRef(0);

  useEffect(() => {
    if (!active || operation !== null) return;
    let cancelled = false;
    let timer = 0;
    const version = actionVersion.current;
    const poll = async () => {
      const next = await getClashStatus().catch(() => null);
      if (cancelled || version !== actionVersion.current) return;
      setStatus(next);
      timer = window.setTimeout(poll, 1_000);
    };
    void poll();
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [active, operation]);

  const running = Boolean(status?.running);
  const unsupported = status?.supported === false;
  const needsConfig = status?.installed === false && !running;
  const activity = status === undefined ? "正在检测"
    : status === null ? "状态暂不可用"
    : unsupported ? "仅支持 Windows"
    : status.running ? "运行中"
    : status.installed ? "未运行" : "未检测到程序";
  const mixedPort = running ? status?.mixedPort : null;
  const subscription = status?.subscription;
  const hasSubscription = subscription != null && Number.isFinite(subscription.used) &&
    Number.isFinite(subscription.total) && subscription.used >= 0 && subscription.total > 0;
  const download = running ? status?.traffic?.downloadSpeed : undefined;
  const upload = running ? status?.traffic?.uploadSpeed : undefined;
  const location = running ? status?.ipLocation : null;

  const run = async (
    kind: ClashToggleKind | "open" | "mode",
    action: () => Promise<ClashStatus | void>,
  ) => {
    if (actionInFlight.current) return;
    actionInFlight.current = true;
    actionVersion.current += 1;
    setOperation(kind);
    try {
      const next = await action();
      if (next) setStatus(next);
    } catch (error) {
      onError(String(error));
    } finally {
      actionInFlight.current = false;
      setOperation(null);
    }
  };

  return (
    <section className="clash-panel" aria-label="Clash Verge">
      <header className="clash-heading">
        <span className="clash-mark"><Icon name="globe" size={17} /></span>
        <h3>Clash Verge</h3>
        <span className={`clash-activity ${running ? "running" : ""}`} role="status">{activity}</span>
        {mixedPort != null && <span className="clash-details">端口 {mixedPort}</span>}
        {!unsupported && (
          <button
            type="button"
            className="mini-text-btn clash-open"
            onClick={needsConfig ? onConfigure : () => void run("open", openClashVerge)}
            disabled={operation !== null}
            aria-label={needsConfig ? "配置 Clash Verge" : "打开 Clash Verge"}
          >
            <Icon name={needsConfig ? "settings" : "external"} size={13} />
            {operation === "open" ? "正在打开" : needsConfig ? "设置" : "打开"}
          </button>
        )}
      </header>

      <div className="clash-metrics">
        <div className="clash-usage">
          <span className="clash-metric-label">订阅流量</span>
          {hasSubscription ? (
            <>
              <span className="clash-metric-value">
                <b>{formatTraffic(subscription.used)}</b> / {formatTraffic(subscription.total)}
              </span>
              <progress
                className="clash-progress"
                aria-label="已使用订阅流量"
                aria-valuetext={`已用 ${formatTraffic(subscription.used)}，总量 ${formatTraffic(subscription.total)}`}
                value={Math.min(subscription.used, subscription.total)}
                max={subscription.total}
              />
            </>
          ) : <span className="clash-metric-value">暂无流量配额</span>}
        </div>
        <div className="clash-speed">
          <span className="clash-metric-label"><Icon name="arrowUp" size={13} /> 上传速度</span>
          <b className="clash-metric-value">{formatSpeed(upload)}</b>
        </div>
        <div className="clash-speed">
          <span className="clash-metric-label"><Icon name="arrowDown" size={13} /> 下载速度</span>
          <b className="clash-metric-value">{formatSpeed(download)}</b>
        </div>
      </div>

      <div className="clash-location">
        <span className="clash-metric-label"><Icon name="mapPin" size={13} /> 出口 IP</span>
        {location?.ip ? (
          <span className="clash-address" title={[location.ip, location.region].filter(Boolean).join(" · ")}>
            <b>{location.ip}</b><span>{location.region || "地区未知"}</span>
          </span>
        ) : <span className="clash-location-empty">暂不可用</span>}
      </div>

      <div className="clash-mode">
        <span className="clash-metric-label">代理模式</span>
        <div className="segmented" role="group" aria-label="代理模式">
          {MODES.map(({ value, label }) => {
            const selected = status?.mode === value;
            return (
              <button
                key={value}
                type="button"
                className={selected ? "active" : ""}
                aria-pressed={selected}
                disabled={!status?.canSetMode || operation !== null}
                onClick={() => {
                  if (!selected) void run("mode", () => setClashMode(value));
                }}
              >
                {label}
              </button>
            );
          })}
        </div>
        <span className="clash-switch-state" role="status">
          {operation === "mode" ? "切换中" : status?.mode === "script" ? "脚本模式" : null}
        </span>
      </div>

      <div className="clash-switches">
        {SWITCHES.map(({ kind, label, capability }) => {
          const checked = status?.[kind];
          const canSet = status?.[capability] === true && typeof checked === "boolean";
          return (
            <div key={kind} className="clash-switch">
              <span>{label}</span>
              <span className="clash-switch-state" role="status">
                {operation === kind ? "切换中" : checked == null ? "暂不可用" : checked ? "开启" : "关闭"}
              </span>
              <Toggle
                label={label}
                checked={checked === true}
                disabled={!canSet || operation !== null}
                onChange={(enabled) => void run(kind, () => setClashToggle(kind, enabled))}
              />
            </div>
          );
        })}
      </div>
    </section>
  );
}
