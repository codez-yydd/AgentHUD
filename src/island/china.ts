/** 省 / 市 / 区县三级数据（来源：MIT 许可的 cn-division / 民政部地名服务）。 */

export interface DivisionNode {
  c: string | number;
  n: string;
  ch?: DivisionNode[];
}

// ===== 惰性动态加载（内存压降专项）=====
// china-pca.json 约 107KB，只有设置页的天气位置级联选择器会用到。原先
// 静态 import 会把它打进 island 主 bundle，岛常驻时这份解析后的 JSON 白占
// 内存。现在改为 loadDivisions() 动态 import（Vite 拆独立 chunk，仅设置
// 页天气区块挂载时触发一次加载），同步查询函数读缓存：未加载完成时返回
// 空数据（选择器短暂显示"选择省份"占位），加载到达后由组件侧 ready 标记
// 驱动重渲染补齐。真机上用户几乎不进设置页改城市，主 bundle 与常驻内存
// 都不再包含这份数据。
let CACHE: DivisionNode[] | null = null;
let LOADING: Promise<DivisionNode[]> | null = null;

/** 动态加载省市区数据（幂等，并发调用共享同一次 import）。 */
export function loadDivisions(): Promise<DivisionNode[]> {
  if (CACHE) return Promise.resolve(CACHE);
  if (!LOADING) {
    LOADING = import("./data/china-pca.json")
      .then((mod) => {
        CACHE = mod.default as DivisionNode[];
        return CACHE;
      })
      .catch((e) => {
        // 失败置回 null 允许下次重试（chunk 下载/解析失败如磁盘抖动）：
        // 否则 rejected promise 被永久缓存，选择器永远无数据
        LOADING = null;
        throw e;
      });
  }
  return LOADING;
}

/** 数据是否已就绪（组件初始化 state 用，避免已加载后的多余重渲染）。 */
export function divisionsLoaded(): boolean {
  return CACHE !== null;
}

function divisions(): DivisionNode[] {
  return CACHE ?? [];
}

function bare(name: string): string {
  return name
    .replace(/(特别行政区|维吾尔自治区|壮族自治区|回族自治区|自治区|自治州|地区|省|市|区|县|盟)$/u, "")
    .trim();
}

function containsName(value: string, target: string): boolean {
  const a = bare(value);
  const b = bare(target);
  return Boolean(a && b) && (a === b || a.includes(b) || b.includes(a));
}

export function provinces(): DivisionNode[] {
  return divisions();
}

export function citiesOf(provinceCode: string): DivisionNode[] {
  return divisions().find((p) => String(p.c) === provinceCode)?.ch ?? [];
}

export function districtsOf(provinceCode: string, cityCode: string): DivisionNode[] {
  return citiesOf(provinceCode).find((c) => String(c.c) === cityCode)?.ch ?? [];
}

export function findDivisionPath(
  provinceName: string,
  cityName: string,
  label = ""
): { province?: DivisionNode; city?: DivisionNode; district?: DivisionNode } {
  const list = divisions();
  const province =
    list.find((p) => containsName(p.n, provinceName)) ??
    list.find((p) => containsName(p.n, label)) ??
    list.find((p) => (p.ch ?? []).some((c) => containsName(c.n, cityName)));
  if (!province) return {};
  const city =
    province.ch?.find((c) => containsName(c.n, cityName)) ??
    province.ch?.find((c) => (label ? label.includes(bare(c.n)) : false)) ??
    province.ch?.[0];
  if (!city) return { province };
  const district = city.ch?.find((d) => (label ? label.includes(bare(d.n)) : false));
  return { province, city, district };
}

/** 当前位置的省/市名 → 三级数据里的节点（IP 库给“陕西/西安”，数据里是“陕西省/西安市”）。 */
export function matchPlace(provinceName: string, cityName: string): ReturnType<typeof findDivisionPath> {
  return findDivisionPath(provinceName, cityName);
}
