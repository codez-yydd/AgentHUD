import type { RequestLogRow } from "./types";

/** Backend owns both the rate and its validity. Null means unavailable. */
export function speedOf(
  row: RequestLogRow
): { tps: number; approximate: boolean } | null {
  if (row.speed_tps == null || !Number.isFinite(row.speed_tps) || row.speed_tps <= 0 ||
      (row.speed_quality !== "generation" && row.speed_quality !== "request_average")) return null;
  return { tps: row.speed_tps, approximate: row.speed_quality === "request_average" };
}
