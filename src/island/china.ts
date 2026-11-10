/** 省 / 市 / 区县三级数据（来源：MIT 许可的 cn-division / 民政部地名服务）。 */

import raw from "./data/china-pca.json";

export interface DivisionNode {
  c: string | number;
  n: string;
  ch?: DivisionNode[];
}

const PROVINCES = raw as DivisionNode[];

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
  return PROVINCES;
}

export function citiesOf(provinceCode: string): DivisionNode[] {
  return PROVINCES.find((p) => String(p.c) === provinceCode)?.ch ?? [];
}

export function districtsOf(provinceCode: string, cityCode: string): DivisionNode[] {
  return citiesOf(provinceCode).find((c) => String(c.c) === cityCode)?.ch ?? [];
}

export function findDivisionPath(
  provinceName: string,
  cityName: string,
  label = ""
): { province?: DivisionNode; city?: DivisionNode; district?: DivisionNode } {
  const province =
    PROVINCES.find((p) => containsName(p.n, provinceName)) ??
    PROVINCES.find((p) => containsName(p.n, label)) ??
    PROVINCES.find((p) => (p.ch ?? []).some((c) => containsName(c.n, cityName)));
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
