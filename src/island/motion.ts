/** Spring motion, drag-slop and screen-edge snapping helpers. */
import { gapToEdge as screenGap } from "./liquid";
import type { IslandBodyRect, IslandEdge } from "./types";

export const ISLAND_EDGES = ["top", "bottom", "left", "right"] as const;
export const DRAG_SLOP = 10;
export const LONG_PRESS_MS = 420;

export function hasDragMovement(dx: number, dy: number, navigation: boolean) {
  return navigation ? Math.abs(dx) + Math.abs(dy) > 4 : Math.hypot(dx, dy) > DRAG_SLOP;
}

export interface Spring {
  value: number;
  velocity: number;
}

export function advanceSpring(
  spring: Spring,
  target: number,
  stiffness: number,
  ratio: number,
  dt: number,
  epsilon = 0.05,
) {
  if (Math.abs(spring.value - target) < epsilon && Math.abs(spring.velocity) < epsilon * 8) {
    spring.value = target;
    spring.velocity = 0;
    return;
  }
  const damping = 2 * ratio * Math.sqrt(stiffness);
  const steps = Math.max(1, Math.ceil(dt * 240));
  const step = dt / steps;
  for (let i = 0; i < steps; i += 1) {
    spring.velocity += (-stiffness * (spring.value - target) - damping * spring.velocity) * step;
    spring.value += spring.velocity * step;
  }
}

export const springAt = (value: number, velocity = 0): Spring => ({ value, velocity });

export { screenGap };

export function nearestScreenEdge(
  body: IslandBodyRect,
  lines: Record<IslandEdge, number>,
  sideWeight = 1,
) {
  let best: IslandEdge = "top";
  let score = Infinity;
  for (const edge of ISLAND_EDGES) {
    const distance = screenGap(edge, body, lines[edge]);
    const weighted = edge === "left" || edge === "right" ? distance * sideWeight : distance;
    if (weighted < score) {
      score = weighted;
      best = edge;
    }
  }
  return best;
}
