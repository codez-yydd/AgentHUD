/**
 * Liquid geometry: path calculations for the capsule body, liquid bridge and edge drip.
 */

export const CANVAS_W = 780;
export const CANVAS_H = 632;
export const INSET = 70;
export const FILLET = 13;
export const BREAK = 64;
export const JOIN = 30;
export const EDGE_MARGIN = 6;
export const K = 0.5523;

export type LiquidEdge = "top" | "bottom" | "left" | "right";

export interface LiquidDrip {
  center: number;
  radius: number;
  edge?: LiquidEdge;
  line?: number;
}

interface BodyRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface LiquidInput {
  edge: LiquidEdge;
  line: number;
  body: BodyRect;
  gap: number;
  neck: number;
  dock: number;
  fillet?: number;
  drips?: LiquidDrip[];
  poke?: number;
  pokePoint?: { x: number; y: number };
  wave?: { amplitude: number; time: number };
}

type Point = [number, number];
type Radii = [number, number, number, number];
type Cubic = [Point, Point, Point, Point];
type Segment = [Point, Point] | Cubic;
type Contour = Segment[];

const clamp = (x: number, lo: number, hi: number) => Math.max(lo, Math.min(hi, x));
const lerp = (a: number, b: number, t: number) => a + (b - a) * t;
const smooth = (value: number) => {
  const t = clamp(value, 0, 1);
  return t * t * (3 - 2 * t);
};

/** Give any nonzero audio peak a visible baseline while keeping loud peaks bounded. */
export function audioMotionLevel(peak: number): number {
  if (!Number.isFinite(peak) || peak <= 0) return 0;
  return 0.42 + 0.58 * Math.min(1, peak) ** 0.45;
}

const edges: LiquidEdge[] = ["top", "bottom", "left", "right"];
const far: Record<LiquidEdge, Point> = {
  top: [0, 1], bottom: [0, -1], left: [1, 0], right: [-1, 0],
};
export const isHorizontalEdge = (edge: LiquidEdge) => edge === "top" || edge === "bottom";

export function edgeLine(edge: LiquidEdge): number {
  if (edge === "top" || edge === "left") return INSET;
  return edge === "bottom" ? CANVAS_H - INSET : CANVAS_W - INSET;
}

export function gapToEdge(edge: LiquidEdge, body: BodyRect, line = edgeLine(edge)): number {
  if (edge === "top") return body.y - line;
  if (edge === "bottom") return line - (body.y + body.height);
  if (edge === "left") return body.x - line;
  return line - (body.x + body.width);
}

export function nearestEdge(body: BodyRect, lines?: Partial<Record<LiquidEdge, number>>) {
  return edges.reduce((nearest, edge) => {
    const gap = gapToEdge(edge, body, lines?.[edge]);
    return gap < nearest.gap ? { edge, gap } : nearest;
  }, { edge: edges[0], gap: Infinity });
}

export function bodyRadius(body: BodyRect): number {
  return Math.min(body.height / 2, body.width / 2, 30);
}

function edgeMap(edge: LiquidEdge, line: number): (u: number, v: number) => Point {
  if (edge === "top") return (u, v) => [u, line + v];
  if (edge === "bottom") return (u, v) => [u, line - v];
  if (edge === "left") return (u, v) => [line + v, u];
  return (u, v) => [line - v, u];
}

function screenWinding(edge: LiquidEdge, contour: Contour): Contour {
  if (edge === "top" || edge === "right") return contour;
  return contour.reverse().map((segment): Segment => segment.length === 2
    ? [segment[1], segment[0]]
    : [segment[3], segment[2], segment[1], segment[0]]);
}

function contourAt(start: Point) {
  const segments: Contour = [];
  let current = start;

  return {
    line(end: Point) {
      if (current[0] !== end[0] || current[1] !== end[1]) segments.push([current, end]);
      current = end;
    },
    cubic(c1: Point, c2: Point, end: Point) {
      segments.push([current, c1, c2, end]);
      current = end;
    },
    close() {
      this.line(start);
      return segments;
    },
  };
}

function shellPath(input: LiquidInput, radius: number, dock: number): Contour {
  const { body, edge, fillet = FILLET } = input;
  const horizontal = isHorizontalEdge(edge);
  const length = horizontal ? body.width : body.height;
  const depth = horizontal ? body.height : body.width;
  const start = horizontal ? body.x : body.y;
  const line = edge === "top" ? body.y : edge === "bottom" ? body.y + depth
    : edge === "left" ? body.x : body.x + depth;
  const map = edgeMap(edge, line);
  const at = (u: number, v: number) => map(start + u, v);
  // The same cubic goes from a convex corner to a concave shoulder as its
  // start crosses the side of the body. Its tangents remain aligned throughout.
  const near = lerp(radius, -fillet, dock);
  const rise = lerp(radius, Math.min(fillet, depth - radius), dock);
  const lip = 1.5 * dock;
  const path = contourAt(at(near, -lip));
  path.line(at(length - near, -lip));
  path.line(at(length - near, 0));
  path.cubic(at(length - near * (1 - K), 0), at(length, rise * (1 - K)), at(length, rise));
  path.line(at(length, depth - radius));
  path.cubic(at(length, depth - radius * (1 - K)), at(length - radius * (1 - K), depth), at(length - radius, depth));
  path.line(at(radius, depth));
  path.cubic(at(radius * (1 - K), depth), at(0, depth - radius * (1 - K)), at(0, depth - radius));
  path.line(at(0, rise));
  path.cubic(at(0, rise * (1 - K)), at(near * (1 - K), 0), at(near, 0));
  return screenWinding(edge, path.close());
}

function sampleContour(contour: Contour): Point[] {
  const points: Point[] = [];
  for (const segment of contour) {
    const length = segment.slice(1).reduce((sum, [x, y], i) =>
      sum + Math.hypot(x - segment[i][0], y - segment[i][1]), 0);
    const count = Math.max(1, Math.ceil(length / 4));
    for (let i = 0; i < count; i += 1) {
      const t = i / count;
      const point = segment.length === 2 ? segment[0].map((v, axis) => lerp(v, segment[1][axis], t))
        : segment[0].map((v, axis) => (1 - t) ** 3 * v + 3 * (1 - t) ** 2 * t * segment[1][axis]
          + 3 * (1 - t) * t ** 2 * segment[2][axis] + t ** 3 * segment[3][axis]);
      points.push([point[0], point[1]]);
    }
  }
  return points;
}

function deform(points: Point[], input: LiquidInput): Point[] {
  const { body, edge, wave, pokePoint } = input;
  const [farX, farY] = far[edge];
  const horizontal = isHorizontalEdge(edge);
  const cx = body.x + body.width / 2;
  const cy = body.y + body.height / 2;
  const amp = clamp(input.poke ?? 0, -0.6, 1.2) * 8;
  const center = horizontal ? cx : cy;
  const half = Math.max(20, (horizontal ? body.width : body.height) / 2);
  const depth = horizontal ? body.height : body.width;
  const near = (horizontal ? cy : cx) - (horizontal ? farY : farX) * depth / 2;
  const pokeU = pokePoint
    ? clamp(horizontal ? pokePoint.x : pokePoint.y, center - half, center + half)
    : center;

  return points.map(([x, y]) => {
    const rx = x - cx;
    const ry = y - cy;
    if (wave) {
      const u = horizontal ? x : y;
      const face = smooth((rx * farX + ry * farY) / (depth / 2));
      const endFade = 1 - smooth((Math.abs(u - center) / half - 0.65) / 0.35);
      const offset = clamp(wave.amplitude, 0, 1) * 3.2 * face * endFade *
        Math.sin(u * 0.075 - wave.time * 7);
      x += farX * offset;
      y += farY * offset;
    }
    if (Math.abs(input.poke ?? 0) > 0.01 && pokePoint) {
      const v = Math.abs((horizontal ? y : x) - near);
      const weight = clamp((v - depth * 0.3) / (depth * 0.7), 0, 1);
      const distance = ((horizontal ? x : y) - pokeU) / (half * 0.5);
      const gaussian = Math.exp(-0.5 * distance * distance);
      const offset = amp * weight ** 2 * (-gaussian + 0.35 * (1 - gaussian));
      x += farX * offset;
      y += farY * offset;
    }
    return [x, y];
  });
}

/** Native glass follows the body. Reserve space for the SVG-only inward deformations. */
export function nativeGlassBody(input: LiquidInput, radii: Radii): BodyRect & { radii: Radii } {
  const { body, edge, poke = 0, wave } = input;
  const normalShift = Math.max(0, poke, -0.35 * poke) * 8 + (wave?.amplitude ?? 0) * 3.2;
  const horizontal = isHorizontalEdge(edge);
  const insetX = horizontal ? 0 : Math.min(body.width / 4, normalShift);
  const insetY = horizontal ? Math.min(body.height / 4, normalShift) : 0;
  return {
    x: body.x + insetX,
    y: body.y + insetY,
    width: body.width - 2 * insetX,
    height: body.height - 2 * insetY,
    radii,
  };
}

function polyPath(points: Point[]): Contour {
  const path = contourAt(points[0]);
  const n = points.length;
  for (let i = 0; i < n; i += 1) {
    const p0 = points[(i + n - 1) % n];
    const p1 = points[i];
    const p2 = points[(i + 1) % n];
    const p3 = points[(i + 2) % n];
    path.cubic(
      [p1[0] + (p2[0] - p0[0]) / 6, p1[1] + (p2[1] - p0[1]) / 6],
      [p2[0] - (p3[0] - p1[0]) / 6, p2[1] - (p3[1] - p1[1]) / 6],
      p2,
    );
  }
  return path.close();
}

function bridgePath(input: LiquidInput, radius: number): Contour | null {
  const { body, edge, line, gap, neck } = input;
  const m = edgeMap(edge, line);
  const horizontal = isHorizontalEdge(edge);
  const center = horizontal ? body.x + body.width / 2 : body.y + body.height / 2;
  const length = horizontal ? body.width : body.height;
  // Taper at contact and before breakup. The bridge never reaches the corners,
  // and its closing edge is buried inside the body, past the rounded end.
  const strength = clamp(neck, 0, 1) * smooth(gap / 8) * (1 - smooth((gap - JOIN) / (BREAK - JOIN)));
  const span = Math.min(length * 0.23, 32) * strength;
  if (span < 0.05) return null;
  const at = (u: number, v: number) => m(center + u, v);
  const waist = span * 0.42;
  const middle = gap * 0.55;
  const end = gap + radius;
  const path = contourAt(at(-span, -1.5));
  path.line(at(span, -1.5));
  path.line(at(span, 0));
  path.cubic(at(waist, 0), at(waist, middle * 0.55), at(waist, middle));
  path.cubic(at(waist, gap), at(span, gap), at(span, end));
  path.line(at(-span, end));
  path.cubic(at(-span, gap), at(-waist, gap), at(-waist, middle));
  path.cubic(at(-waist, middle * 0.55), at(-waist, 0), at(-span, 0));
  return screenWinding(edge, path.close());
}

function dripPath(edge: LiquidEdge, line: number, center: number, r: number): Contour {
  const m = edgeMap(edge, line);
  const at = (u: number, v: number) => m(center + u, v);
  const f = r * 0.8;
  const path = contourAt(at(-r - f, -1.5));
  path.line(at(r + f, -1.5));
  path.line(at(r + f, 0));
  path.cubic(at(r + f * (1 - K), 0), at(r, f * (1 - K)), at(r, f * 0.9));
  path.cubic(at(r, f + r * 0.9), at(-r, f + r * 0.9), at(-r, f * 0.9));
  path.cubic(at(-r, f * (1 - K)), at(-r - f * (1 - K), 0), at(-r - f, 0));
  return path.close();
}

function serialize(contour: Contour): string {
  const xy = ([x, y]: Point) => `${x.toFixed(2)} ${y.toFixed(2)}`;
  const commands = contour.map((segment) => segment.length === 2
    ? `L${xy(segment[1])}`
    : `C${xy(segment[1])} ${xy(segment[2])} ${xy(segment[3])}`);
  return `M${xy(contour[0][0])}${commands.join("")}Z`;
}

export function liquidPaths(input: LiquidInput) {
  const { body, edge, line, dock, fillet = FILLET } = input;
  const radius = bodyRadius(body);
  // Once the shoulders turn concave, the shell covers the whole rectangle.
  // Only then can the optical backing fill the near corners without spilling past it.
  const concaveAt = radius / (radius + fillet);
  const near = radius * (1 - smooth((dock - concaveAt) / (1 - concaveAt)));
  const glassRadii: Radii = [
    edge === "top" || edge === "left" ? near : radius,
    edge === "top" || edge === "right" ? near : radius,
    edge === "bottom" || edge === "right" ? near : radius,
    edge === "bottom" || edge === "left" ? near : radius,
  ];
  const deformed = Boolean(input.wave) || Math.abs(input.poke ?? 0) > 0.01;
  const drawShell = (amount: number) => {
    const contour = shellPath(input, radius, amount);
    return serialize(deformed ? polyPath(deform(sampleContour(contour), input)) : contour);
  };
  const bodyClip = drawShell(0);
  const rim = dock > 0 ? drawShell(dock) : bodyClip;
  const effects: Contour[] = [];
  const bridge = bridgePath(input, radius);
  if (bridge) effects.push(bridge);
  for (const drip of input.drips ?? []) {
    if (drip.radius <= 0.4) continue;
    const dripEdge = drip.edge ?? edge;
    const dripLine = drip.line ?? (dripEdge === edge ? line : edgeLine(dripEdge));
    effects.push(screenWinding(dripEdge, dripPath(dripEdge, dripLine, drip.center, drip.radius)));
  }
  // Nonzero winding fills the bridge and body once where they overlap. Only
  // the shell gets a stroke so the hidden end of an effect never draws a seam.
  return { radius, bodyClip, rim, glassClip: [rim, ...effects.map(serialize)].join(" "), glassRadii };
}

export function cssPath(d: string): string {
  return `path("${d}")`;
}
