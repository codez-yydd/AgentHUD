/**
 * 主窗口设置页共享控件：ToggleSwitch 自 SettingsPage 提取，供设置页
 * 既有行与新增分区（通用 / 悬浮窗 / 数据来源）共用同一胶囊滑动开关。
 */

/** 开关行控件：胶囊滑动开关（状态即语义，title 给可访问名） */
export function ToggleSwitch({
  checked,
  disabled,
  title,
  onChange,
}: {
  checked: boolean;
  disabled?: boolean;
  title: string;
  onChange: (next: boolean) => void;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={title}
      title={title}
      disabled={disabled}
      className={`db-set-switch ${checked ? "db-set-switch--on" : ""}`}
      onClick={() => onChange(!checked)}
    >
      <span className="db-set-switch__knob" aria-hidden />
    </button>
  );
}
