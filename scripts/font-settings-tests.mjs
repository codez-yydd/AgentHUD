/**
 * 字号配置迁移与校验验证：
 *   node --experimental-strip-types scripts/font-settings-tests.mjs
 *
 * 覆盖：旧 zbar-ui-scale 档位等比迁移、主窗口默认不跟随、联动解析、
 * 边界夹取、展示格式、0.5px 步进、持久化往返与损坏值回退。
 */
import assert from "node:assert/strict";

const store = new Map();
globalThis.localStorage = {
  getItem: (key) => (store.has(key) ? store.get(key) : null),
  setItem: (key, value) => store.set(key, String(value)),
  removeItem: (key) => store.delete(key),
  clear: () => store.clear(),
};

const font = await import("../src/fontSettings.ts");
const {
  DEFAULT_FONT_PX,
  FONT_MAX_PX,
  FONT_MIN_PX,
  fontScaleOf,
  formatFontPx,
  isValidFontPx,
  loadDashboardFontLink,
  loadDashboardFontPx,
  loadPanelFontPx,
  persistDashboardFontLink,
  persistDashboardFontPx,
  persistPanelFontPx,
  resolveFontPx,
  sanitizeFontPx,
  stepFontPx,
} = font;

let passed = 0;
function test(name, fn) {
  try {
    fn();
    passed += 1;
    console.log("ok  ", name);
  } catch (error) {
    console.error("FAIL", name);
    throw error;
  }
}

test("旧整体缩放档位按 13px 基准等比迁移", () => {
  const cases = [
    ["0.9", 11.7],
    ["1", 13],
    ["1.1", 14.3],
    ["1.25", 16.25],
  ];
  for (const [legacy, px] of cases) {
    store.clear();
    store.set("zbar-ui-scale", legacy);
    assert.equal(loadPanelFontPx(), px, legacy);
    assert.equal(fontScaleOf(loadPanelFontPx()), Number(legacy), legacy);
  }
});

test("主窗口默认 13px，不被面板旧档位连带放大", () => {
  store.clear();
  store.set("zbar-ui-scale", "1.25");
  assert.equal(loadPanelFontPx(), 16.25);
  assert.equal(loadDashboardFontPx(), DEFAULT_FONT_PX);
  assert.equal(loadDashboardFontLink(), false);
  assert.equal(resolveFontPx("dashboard"), DEFAULT_FONT_PX);
});

test("联动开关：主窗口解析为面板字号，关闭后恢复自身保存值", () => {
  store.clear();
  persistPanelFontPx(17);
  persistDashboardFontPx(14);
  persistDashboardFontLink(true);
  assert.equal(resolveFontPx("dashboard"), 17);
  persistDashboardFontLink(false);
  assert.equal(resolveFontPx("dashboard"), 14);
});

test("字号边界：越界夹取，非数字回退默认", () => {
  assert.equal(sanitizeFontPx(9), FONT_MIN_PX);
  assert.equal(sanitizeFontPx(30), FONT_MAX_PX);
  assert.equal(sanitizeFontPx("abc"), DEFAULT_FONT_PX);
  assert.equal(sanitizeFontPx(16.256), 16.26);
  assert.equal(isValidFontPx(10), true);
  assert.equal(isValidFontPx(24), true);
  assert.equal(isValidFontPx(9.9), false);
  assert.equal(isValidFontPx(24.1), false);
});

test("展示格式保留旧档位精度（16.25 不四舍五入成 16.3）", () => {
  assert.equal(formatFontPx(13), "13");
  assert.equal(formatFontPx(11.7), "11.7");
  assert.equal(formatFontPx(16.25), "16.25");
  assert.equal(formatFontPx(18.5), "18.5");
});

test("0.5px 步进：吸附网格后增减，不越界", () => {
  assert.equal(stepFontPx(13, 1), 13.5);
  assert.equal(stepFontPx(13, -1), 12.5);
  assert.equal(stepFontPx(11.7, 1), 12);
  assert.equal(stepFontPx(11.7, -1), 11);
  assert.equal(stepFontPx(FONT_MIN_PX, -1), FONT_MIN_PX);
  assert.equal(stepFontPx(FONT_MAX_PX, 1), FONT_MAX_PX);
});

test("持久化往返：新键优先于旧键，坏值回退默认", () => {
  store.clear();
  store.set("zbar-ui-scale", "1.25");
  persistPanelFontPx(18.5);
  assert.equal(loadPanelFontPx(), 18.5);
  store.set("zbar-font-panel", "oops");
  assert.equal(loadPanelFontPx(), 16.25);
  store.set("zbar-font-panel", "30");
  assert.equal(loadPanelFontPx(), 16.25);
});

test("scale 与基准字号线性对应", () => {
  assert.equal(fontScaleOf(13), 1);
  assert.equal(fontScaleOf(16.25), 1.25);
  assert.equal(fontScaleOf(24), Math.round((24 / 13) * 10000) / 10000);
});

console.log(`\n${passed} font tests passed`);
