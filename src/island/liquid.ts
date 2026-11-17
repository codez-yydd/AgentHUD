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
  fillet?: number;
  drips?: LiquidDrip[];
  poke?: number;
  pokePoint?: { x: number; y: number };
  vx?: number;
  vy?: number;
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
const edgeCorners: Record<LiquidEdge, [boolean, boolean, boolean, boolean]> = {
  top: [true, true, false, false],
  bottom: [false, false, true, true],
  left: [true, false, false, true],
  right: [false, true, true, false],
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

export function liquidRadii(body: BodyRect, edge: LiquidEdge, gap: number, neck: number): Radii {
  const base = bodyRadius(body);
  const corner = lerp(base, base * smooth(clamp(gap / BREAK, 0, 1) * 1.8), neck);
  const [tl, tr, br, bl] = edgeCorners[edge];
  return [tl ? corner : base, tr ? corner : base, br ? corner : base, bl ? corner : base];
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

function bodyPath({ x, y, width: w, height: h }: BodyRect, [tl, tr, br, bl]: Radii): Contour {
  const path = contourAt([x + tl, y]);
  path.line([x + w - tr, y]);
  if (tr) path.cubic([x + w - tr + tr * K, y], [x + w, y + tr - tr * K], [x + w, y + tr]);
  path.line([x + w, y + h - br]);
  if (br) path.cubic([x + w, y + h - br + br * K], [x + w - br + br * K, y + h], [x + w - br, y + h]);
  path.line([x + bl, y + h]);
  if (bl) path.cubic([x + bl - bl * K, y + h], [x, y + h - bl + bl * K], [x, y + h - bl]);
  path.line([x, y + tl]);
  if (tl) path.cubic([x, y + tl - tl * K], [x + tl - tl * K, y], [x + tl, y]);
  return path.close();
}

function bodyPoints({ x, y, width: w, height: h }: BodyRect, [tl, tr, br, bl]: Radii): Point[] {
  const points: Point[] = [];
  const step = 4;

  function line(x0: number, y0: number, x1: number, y1: number) {
    const n = Math.max(1, Math.floor(Math.hypot(x1 - x0, y1 - y0) / step));
    for (let i = 0; i < n; i += 1) points.push([lerp(x0, x1, i / n), lerp(y0, y1, i / n)]);
  }

  function arc(cx: number, cy: number, radius: number, from: number) {
    if (radius <= 0.01) {
      points.push([cx, cy]);
      return;
    }
    const n = Math.max(2, Math.floor(radius * Math.PI / (2 * step)));
    for (let i = 0; i < n; i += 1) {
      const angle = from + Math.PI * i / (2 * n);
      points.push([cx + radius * Math.cos(angle), cy + radius * Math.sin(angle)]);
    }
  }

  line(x + tl, y, x + w - tr, y);
  arc(x + w - tr, y + tr, tr, -Math.PI / 2);
  line(x + w, y + tr, x + w, y + h - br);
  arc(x + w - br, y + h - br, br, 0);
  line(x + w - br, y + h, x + bl, y + h);
  arc(x + bl, y + h - bl, bl, Math.PI / 2);
  line(x, y + h - bl, x, y + tl);
  arc(x + tl, y + tl, tl, Math.PI);
  // A corner starting to round can sample the same subpixel position twice.
  // Merge these samples before fitting splines to avoid tiny loops.
  return points.filter(([x, y], i) => {
    const [px, py] = points[(i + points.length - 1) % points.length];
    return Math.hypot(x - px, y - py) > 0.05;
  });
}

function dragDeformation(body: BodyRect, vx: number, vy: number, speed: number) {
  const amt = clamp(speed / 2600, 0, 1);
  const [dx, dy] = speed > 1e-3 ? [vx / speed, vy / speed] : [0, 0];
  const cx = body.x + body.width / 2;
  const cy = body.y + body.height / 2;
  const lag = (Math.min(body.width, body.height) / 2 + 12) * 0.6 * amt;
  return { amt, dx, dy, cx, cy, lag };
}

function deform(points: Point[], input: LiquidInput, speed: number): Point[] {
  const { body, edge, vx = 0, vy = 0, wave, pokePoint } = input;
  const [farX, farY] = far[edge];
  const horizontal = isHorizontalEdge(edge);
  const { amt, dx, dy, cx, cy, lag } = dragDeformation(body, vx, vy, speed);
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
    const d = Math.hypot(rx, ry) || 1;
    const nx = rx / d;
    const ny = ry / d;

    if (amt >= 0.004) {
      const tail = clamp((1 - (nx * dx + ny * dy)) * 0.5, 0, 1);
      const along = rx * dx + ry * dy;
      const squeeze = 1 - 0.38 * amt * tail ** 1.2;
      const push = -lag * tail ** 1.6 + lag * 0.12 * (1 - tail) ** 2;
      x = cx + along * dx + (rx - along * dx) * squeeze + dx * push;
      y = cy + along * dy + (ry - along * dy) * squeeze + dy * push;
    }
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
  const { body, edge, vx = 0, vy = 0, poke = 0, wave } = input;
  const speed = Math.hypot(vx, vy);
  const { amt, dx, dy, lag } = dragDeformation(body, vx, vy, speed > 30 ? speed : 0);
  const alongX = Math.abs(dx);
  const alongY = Math.abs(dy);
  const crossExtent = (alongY * body.width + alongX * body.height) / 2;
  const squeeze = crossExtent * 0.38 * amt;
  const normalShift = Math.max(0, poke, -0.35 * poke) * 8 + (wave?.amplitude ?? 0) * 3.2;
  const horizontal = isHorizontalEdge(edge);
  const insetX = Math.min(body.width / 4, alongY * squeeze + alongX * lag + (horizontal ? 0 : normalShift));
  const insetY = Math.min(body.height / 4, alongX * squeeze + alongY * lag + (horizontal ? normalShift : 0));
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

function neckPath(input: LiquidInput, gap: number, corner: number): Contour {
  const { body, edge, line, neck: strength, fillet = FILLET } = input;
  const m = edgeMap(edge, line);
  const horizontal = isHorizontalEdge(edge);
  const center = horizontal ? body.x + body.width / 2 : body.y + body.height / 2;
  const length = horizontal ? body.width : body.height;
  const half = length / 2;
  const t = clamp(gap / BREAK, 0, 1);
  const waist = lerp(1, lerp(half, Math.max(2.5, Math.min(7, length * 0.035)), Math.sqrt(t)), strength);
  const edgeFillet = lerp(fillet, fillet * 0.75, t) * strength;
  const bodyFillet = fillet * smooth(t * 3) * strength;
  const e = waist + edgeFillet;
  const b = Math.min(waist + bodyFillet, Math.max(waist, half - corner * 0.5));
  const overlap = Math.max(Math.min(8, corner + 3), fillet * (1 - t));
  const ym = Math.max(gap * 0.5, edgeFillet);
  const yb = gap + 0.5;

  function side(sign: number) {
    const at = (u: number, v: number) => m(center + sign * u, v);
    const curves: Cubic[] = [[at(e, 0), at(waist + (e - waist) * (1 - K), 0),
      at(waist, ym * (1 - K)), at(waist, ym)]];
    if (yb > ym + 0.5) {
      curves.push([at(waist, ym), at(waist, ym + (yb - ym) * K),
        at(b - (b - waist) * K, yb), at(b, yb)]);
    }
    return { curves, end: at(curves.length > 1 ? b : waist, gap + overlap) };
  }

  const right = side(1);
  const left = side(-1);
  const path = contourAt(m(center - e, -1.5));
  path.line(m(center + e, -1.5));
  path.line(m(center + e, 0));
  for (const [, c1, c2, end] of right.curves) path.cubic(c1, c2, end);
  path.line(right.end);
  path.line(left.end);
  path.line(left.curves[left.curves.length - 1][3]);
  for (const [start, c1, c2] of left.curves.reverse()) path.cubic(c2, c1, start);
  return path.close();
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

interface Bounds {
  minX: number;
  maxX: number;
  minY: number;
  maxY: number;
}

function bounds(points: Point[]): Bounds {
  const result = { minX: Infinity, maxX: -Infinity, minY: Infinity, maxY: -Infinity };
  for (const [x, y] of points) {
    result.minX = Math.min(result.minX, x);
    result.maxX = Math.max(result.maxX, x);
    result.minY = Math.min(result.minY, y);
    result.maxY = Math.max(result.maxY, y);
  }
  return result;
}

function overlaps(a: Bounds, b: Bounds): boolean {
  return a.minX <= b.maxX && a.maxX >= b.minX && a.minY <= b.maxY && a.maxY >= b.minY;
}

const cross = (ax: number, ay: number, bx: number, by: number) => ax * by - ay * bx;
const at = (a: Point, b: Point, t: number): Point => [lerp(a[0], b[0], t), lerp(a[1], b[1], t)];

function flatten(path: Contour): Point[] {
  const points: Point[] = [];
  const add = (point: Point) => {
    const last = points[points.length - 1];
    if (!last || Math.hypot(point[0] - last[0], point[1] - last[1]) > 1e-6) points.push(point);
  };
  function curve(start: Point, c1: Point, c2: Point, end: Point, depth = 0) {
    const [dx, dy] = [end[0] - start[0], end[1] - start[1]];
    const flatness = Math.max(
      Math.hypot(c1[0] - lerp(start[0], end[0], 1 / 3), c1[1] - lerp(start[1], end[1], 1 / 3)),
      Math.hypot(c2[0] - lerp(start[0], end[0], 2 / 3), c2[1] - lerp(start[1], end[1], 2 / 3)),
    );
    if (depth >= 8 || (flatness < 0.12 && dx * dx + dy * dy > 1e-12)) {
      add(start);
      return;
    }
    const a = at(start, c1, 0.5);
    const b = at(c1, c2, 0.5);
    const c = at(c2, end, 0.5);
    const d = at(a, b, 0.5);
    const e = at(b, c, 0.5);
    const middle = at(d, e, 0.5);
    curve(start, a, d, middle, depth + 1);
    curve(middle, e, c, end, depth + 1);
  }

  for (const segment of path) {
    if (segment.length === 2) add(segment[0]);
    else curve(...segment);
  }
  if (points.length > 1 && Math.hypot(
    points[0][0] - points[points.length - 1][0], points[0][1] - points[points.length - 1][1],
  ) < 1e-6) points.pop();
  const winding = points.reduce((area, p, i) => {
    const q = points[(i + 1) % points.length];
    return area + cross(p[0], p[1], q[0], q[1]);
  }, 0);
  return winding < 0 ? points.reverse() : points;
}

function inside([x, y]: Point, polygon: Point[], box: Bounds): boolean {
  if (x < box.minX || x > box.maxX || y < box.minY || y > box.maxY) return false;
  let found = false;
  for (let i = 0, j = polygon.length - 1; i < polygon.length; j = i, i += 1) {
    const [ax, ay] = polygon[j];
    const [bx, by] = polygon[i];
    if ((ay > y) !== (by > y) && x < ax + (bx - ax) * (y - ay) / (by - ay)) found = !found;
  }
  return found;
}

type Line = [Point, Point];

interface Polygon {
  points: Point[];
  bounds: Bounds;
  edges: Line[];
}

function exterior(source: Polygon, targets: Polygon[]): Line[] {
  const result: Line[] = [];
  for (const [a, b] of source.edges) {
    const edgeBounds = bounds([a, b]);
    const nearby = targets.filter((target) => overlaps(edgeBounds, target.bounds));
    if (!nearby.length) {
      result.push([a, b]);
      continue;
    }

    const [dx, dy] = [b[0] - a[0], b[1] - a[1]];
    const length = Math.hypot(dx, dy);
    const cuts = [0, 1];
    for (const target of nearby) {
      for (const [p, q] of target.edges) {
        if ((p[0] > edgeBounds.maxX && q[0] > edgeBounds.maxX) ||
            (p[0] < edgeBounds.minX && q[0] < edgeBounds.minX) ||
            (p[1] > edgeBounds.maxY && q[1] > edgeBounds.maxY) ||
            (p[1] < edgeBounds.minY && q[1] < edgeBounds.minY)) continue;
        const [ex, ey] = [q[0] - p[0], q[1] - p[1]];
        const [rx, ry] = [p[0] - a[0], p[1] - a[1]];
        const determinant = cross(dx, dy, ex, ey);
        if (Math.abs(determinant) > 1e-9) {
          const t = cross(rx, ry, ex, ey) / determinant;
          const u = cross(rx, ry, dx, dy) / determinant;
          if (t > 0 && t < 1 && u >= -1e-9 && u <= 1 + 1e-9) cuts.push(t);
        } else if (Math.abs(cross(rx, ry, dx, dy)) < 1e-9) {
          const project = ([x, y]: Point) => ((x - a[0]) * dx + (y - a[1]) * dy) / length ** 2;
          for (const t of [project(p), project(q)]) {
            if (t > 0 && t < 1) cuts.push(t);
          }
        }
      }
    }

    cuts.sort((a, b) => a - b);
    for (let i = 1; i < cuts.length; i += 1) {
      const from = cuts[i - 1];
      const to = cuts[i];
      if ((to - from) * length < 1e-6) continue;
      const sample = at(a, b, (from + to) / 2);
      // The outward sample handles collinear boundaries shared by the two contours.
      const epsilon = 1e-5 / length;
      sample[0] += dy * epsilon;
      sample[1] -= dx * epsilon;
      if (!nearby.some((target) => inside(sample, target.points, target.bounds))) {
        result.push([at(a, b, from), at(a, b, to)]);
      }
    }
  }
  return result;
}

function united(contours: Contour[]): Contour[] {
  if (contours.length < 2) return contours;
  // Qt unions its painter paths before drawing. Flatten intersecting curves to
  // subpixel tolerance and keep the exterior segments for the single SVG stroke.
  const pieces = contours.map((path) => ({ path, bounds: bounds(path.flat()) }));
  const independent: Contour[] = [];
  const polygons: Polygon[] = [];
  for (const piece of pieces) {
    if (!pieces.some((other) => other !== piece && overlaps(piece.bounds, other.bounds))) {
      independent.push(piece.path);
      continue;
    }
    const points = flatten(piece.path);
    const edges = points.map((a, i): Line => [a, points[(i + 1) % points.length]]);
    polygons.push({ points, edges, bounds: bounds(points) });
  }
  const lines = polygons.flatMap((polygon) =>
    exterior(polygon, polygons.filter((other) => other !== polygon)),
  );
  const key = ([x, y]: Point) => `${Math.round(x * 1e5)},${Math.round(y * 1e5)}`;
  const outgoing = new Map<string, Map<string, Line>>();
  for (const line of lines) {
    const from = key(line[0]);
    const to = key(line[1]);
    if (from === to) continue;
    let exits = outgoing.get(from);
    if (!exits) outgoing.set(from, (exits = new Map()));
    exits.set(to, line);
  }

  while (outgoing.size) {
    const start = outgoing.keys().next().value!;
    let current = start;
    const contour: Contour = [];
    do {
      const exits = outgoing.get(current);
      const next = exits?.entries().next().value;
      if (!exits || !next) return contours;
      const [destination, line] = next;
      contour.push(line);
      exits.delete(destination);
      if (!exits.size) outgoing.delete(current);
      current = destination;
    } while (current !== start);
    if (contour.length >= 3) independent.push(contour);
  }
  return independent;
}

function serialize(contour: Contour): string {
  const xy = ([x, y]: Point) => `${x.toFixed(2)} ${y.toFixed(2)}`;
  const commands = contour.map((segment) => segment.length === 2
    ? `L${xy(segment[1])}`
    : `C${xy(segment[1])} ${xy(segment[2])} ${xy(segment[3])}`);
  return `M${xy(contour[0][0])}${commands.join("")}Z`;
}

export function liquidPath(input: LiquidInput): string {
  const { body, edge, line, neck, vx = 0, vy = 0 } = input;
  if (body.width <= 0 || body.height <= 0) return "";
  const gap = Math.max(0, input.gap);
  const radii = liquidRadii(body, edge, gap, neck);
  const speed = Math.hypot(vx, vy);
  const deformed = speed > 30 || input.wave || Math.abs(input.poke ?? 0) > 0.01;
  const base = deformed
    ? polyPath(deform(bodyPoints(body, radii), input, speed))
    : bodyPath(body, radii);
  const nearCorner = radii[edgeCorners[edge].findIndex(Boolean)];
  const contours = [base];
  if (neck > 0.01) contours.push(screenWinding(edge, neckPath(input, gap, nearCorner)));
  for (const drip of input.drips ?? []) {
    if (drip.radius <= 0.4) continue;
    const dripEdge = drip.edge ?? edge;
    const dripLine = drip.line ?? (dripEdge === edge ? line : edgeLine(dripEdge));
    contours.push(screenWinding(dripEdge, dripPath(dripEdge, dripLine, drip.center, drip.radius)));
  }
  return united(contours).map(serialize).join(" ");
}

export function cssPath(d: string): string {
  return `path("${d}")`;
}
