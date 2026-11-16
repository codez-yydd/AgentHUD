/**
 * CSS font-family helpers shared by the island renderer and its settings window.
 *
 * Font family names come from the installed-font list, so they can contain spaces
 * (and, in theory, CSS string characters). Keep them as one quoted family name
 * before putting them into a CSS custom property or a canvas font shorthand.
 */

/** The island's built-in family list, kept in the same order as island.css. */
export const ISLAND_FONT_STACK =
  '"HarmonyOS Sans", "MiSans", "Segoe UI Variable Text", "Microsoft YaHei UI", "Microsoft YaHei", "Segoe UI", system-ui, sans-serif';

/** The settings window's built-in family list, kept in the same order as its CSS. */
export const SETTINGS_FONT_STACK = ISLAND_FONT_STACK;

/** Numeric controls keep a display-oriented fallback after a selected family. */
export const SETTINGS_NUM_FONT_STACK =
  '"HarmonyOS Sans", "Segoe UI Variable Display", "Segoe UI", system-ui, sans-serif';

/**
 * Quote one font family as a CSS string literal.
 *
 * Control characters are replaced rather than emitted into the declaration;
 * backslashes and quotes are escaped so an installed font name cannot break out
 * of the string when used in an inline style or custom property.
 */
export function quoteFontFamily(family?: string | null): string {
  const value = family?.trim();
  if (!value) return "";
  const escaped = value
    .replace(/[\u0000-\u001f\u007f]/g, " ")
    .replace(/\\/g, "\\\\")
    .replace(/"/g, '\\"');
  return `"${escaped}"`;
}

/**
 * Return a custom family followed by a guaranteed fallback list.
 * `fallback` may itself be a CSS value such as `var(--w-font)`.
 */
export function fontFamilyWithFallback(
  family: string | null | undefined,
  fallback: string
): string {
  const quoted = quoteFontFamily(family);
  return quoted ? `${quoted}, ${fallback}` : fallback;
}
