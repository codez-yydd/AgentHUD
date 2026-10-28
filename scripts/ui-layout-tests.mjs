/**
 * 布局配置关键逻辑验证（无浏览器依赖）：
 *   node --experimental-strip-types scripts/ui-layout-tests.mjs
 *
 * 覆盖：默认布局 / 坏配置回退 / 高度边界 / 未知卡与新卡兼容 / 简洁默认显隐 /
 * 自定义布局的新卡默认隐藏 / 按窗口页面隔离持久化 / 自定义快照保留。
 */
import assert from "node:assert/strict";

// 最小 localStorage 桩（uiLayout 只在读写函数里访问）
const store = new Map();
globalThis.localStorage = {
  getItem: (key) => (store.has(key) ? store.get(key) : null),
  setItem: (key, value) => store.set(key, String(value)),
  removeItem: (key) => store.delete(key),
  clear: () => store.clear(),
};

const layout = await import("../src/uiLayout.ts");
const {
  defaultPageLayout,
  normalizePageLayout,
  readPageLayout,
  writePageLayout,
  readCardState,
  writeCardState,
  defaultCardState,
  readCustomSnapshot,
  saveCustomSnapshot,
  resetPageLayout,
  pageKey,
  MAX_CARD_HEIGHT,
  cardDefinition,
} = layout;

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

test("经典默认：汇总页四张卡、全部自适应、顺序即注册表顺序", () => {
  const page = defaultPageLayout("panel", "summary");
  assert.equal(page.scheme, "classic");
  assert.deepEqual(page.order, [
    "summary.agents",
    "summary.quota",
    "summary.trend",
    "summary.models",
  ]);
  for (const state of Object.values(page.cards)) {
    assert.equal(state.height, null);
    assert.equal(state.hidden, false);
  }
});

test("坏配置逐项回退，未知卡片 ID 被丢弃", () => {
  const page = normalizePageLayout(
    {
      scheme: "bogus",
      order: ["nope", "summary.models", "summary.models", 42],
      cards: {
        "summary.models": { height: "tall", collapsed: "yes", detail: "weird" },
      },
      customized: "yes",
    },
    "panel",
    "summary"
  );
  assert.equal(page.scheme, "classic");
  assert.equal(page.customized, false);
  assert.equal(page.order[0], "summary.models");
  assert.equal(page.order.filter((id) => id === "summary.models").length, 1);
  assert.ok(!page.order.includes("nope"));
  assert.equal(page.cards["summary.models"].height, null);
  assert.equal(page.cards["summary.models"].collapsed, false);
  assert.equal(page.cards["summary.models"].detail, "primary");
  // 缺失的注册表卡片补齐到末尾
  assert.ok(page.order.includes("summary.quota"));
});

test("高度：非数字 / 过小回退自适应，超大值封顶，保留整数", () => {
  const base = defaultPageLayout("panel", "summary");
  const raw = {
    ...base,
    cards: {
      ...base.cards,
      "summary.models": { ...base.cards["summary.models"], height: 99999 },
    },
  };
  assert.equal(
    normalizePageLayout(raw, "panel", "summary").cards["summary.models"].height,
    MAX_CARD_HEIGHT
  );
  for (const bad of [null, "300", -3, 10, NaN, Infinity]) {
    const next = {
      ...base,
      cards: {
        ...base.cards,
        "summary.models": { ...base.cards["summary.models"], height: bad },
      },
    };
    assert.equal(
      normalizePageLayout(next, "panel", "summary").cards["summary.models"].height,
      null,
      `${String(bad)} 应回退自适应`
    );
  }
  const rounded = {
    ...base,
    cards: {
      ...base.cards,
      "summary.models": { ...base.cards["summary.models"], height: 176.6 },
    },
  };
  assert.equal(
    normalizePageLayout(rounded, "panel", "summary").cards["summary.models"].height,
    177
  );
});

test("简洁方案默认只显示套餐余量，其余隐藏", () => {
  const page = defaultPageLayout("panel", "summary", "simple");
  const visible = Object.entries(page.cards)
    .filter(([, state]) => !state.hidden)
    .map(([id]) => id);
  assert.deepEqual(visible, ["summary.quota"]);
});

test("已定制页面：缺失于配置的注册表卡片默认隐藏（新卡不挤进自定义布局）", () => {
  const base = defaultPageLayout("panel", "summary");
  const customized = {
    ...base,
    customized: true,
    order: ["summary.quota", "summary.trend", "summary.models"],
    cards: {
      "summary.quota": { ...base.cards["summary.quota"] },
      "summary.trend": { ...base.cards["summary.trend"] },
      "summary.models": { ...base.cards["summary.models"] },
    },
  };
  const page = normalizePageLayout(customized, "panel", "summary");
  assert.equal(page.customized, true);
  assert.equal(page.cards["summary.quota"].hidden, false);
  assert.equal(page.cards["summary.agents"].hidden, true);
  assert.equal(page.order[page.order.length - 1], "summary.agents");
});

test("持久化按窗口 + 页面隔离，单卡写入不影响其他页", () => {
  store.clear();
  const next = writeCardState("panel", "summary", "summary.models", {
    height: 240,
    listExpanded: true,
  });
  assert.equal(next.height, 240);
  assert.equal(next.listExpanded, true);
  assert.equal(readCardState("panel", "summary", "summary.models").height, 240);
  assert.equal(readCardState("dashboard", "overview", "overview.trend").height, null);
  const stored = JSON.parse(store.get("zbar-ui-layout"));
  assert.ok(stored.pages[pageKey("panel", "summary")]);
  assert.ok(!stored.pages[pageKey("dashboard", "overview")]);
});

test("写入时二次校验坏值，不落盘无效高度", () => {
  store.clear();
  const page = readPageLayout("panel", "summary");
  const dirty = {
    ...page,
    cards: {
      ...page.cards,
      "summary.models": { ...page.cards["summary.models"], height: -100 },
    },
  };
  writePageLayout("panel", "summary", dirty);
  assert.equal(readPageLayout("panel", "summary").cards["summary.models"].height, null);
});

test("自定义快照切换方案后仍可读回", () => {
  store.clear();
  const custom = defaultPageLayout("panel", "summary");
  custom.customized = true;
  custom.order = ["summary.models", "summary.trend", "summary.quota", "summary.agents"];
  custom.cards["summary.models"] = { ...custom.cards["summary.models"], height: 300 };
  saveCustomSnapshot("panel", "summary", custom);
  const restored = readCustomSnapshot("panel", "summary");
  assert.ok(restored);
  assert.equal(restored.order[0], "summary.models");
  assert.equal(restored.cards["summary.models"].height, 300);
});

test("恢复默认清空本页配置与快照", () => {
  store.clear();
  writeCardState("panel", "summary", "summary.models", { height: 260 });
  saveCustomSnapshot("panel", "summary", readPageLayout("panel", "summary"));
  const page = resetPageLayout("panel", "summary");
  assert.equal(page.scheme, "classic");
  assert.equal(page.cards["summary.models"].height, null);
  assert.equal(readCustomSnapshot("panel", "summary"), null);
});

test("每张卡都有注册表定义与合理最小高度", () => {
  for (const id of [
    "summary.agents",
    "summary.quota",
    "summary.trend",
    "summary.models",
    "overview.metrics",
    "overview.trend",
    "overview.heatmap",
    "overview.quota",
    "overview.models",
  ]) {
    const def = cardDefinition(id);
    assert.ok(def, id);
    assert.ok(def.minHeight >= 80, `${id} minHeight`);
    const state = defaultCardState(def, "classic");
    assert.equal(state.hidden, !def.classic);
  }
});

console.log(`\n${passed} layout tests passed`);
