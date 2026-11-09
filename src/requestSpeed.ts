import type { RequestLogRow, SpeedQuality } from "./types";

export type RequestLogSpeedBasis = SpeedQuality | "first_token_estimate";

/** Backend owns both the rate and its validity. Null means unavailable. */
export function speedOf(
  row: RequestLogRow
): { tps: number; basis: RequestLogSpeedBasis } | null {
  if (row.speed_tps == null || !Number.isFinite(row.speed_tps) || row.speed_tps <= 0 ||
      (row.speed_quality !== "generation" && row.speed_quality !== "request_average")) return null;
  const estimate = row.first_token_estimated_tps;
  if (row.source === "zcode" && row.speed_quality === "request_average" &&
      estimate != null && Number.isFinite(estimate) && estimate > 0) {
    return { tps: estimate, basis: "first_token_estimate" };
  }
  return { tps: row.speed_tps, basis: row.speed_quality };
}
