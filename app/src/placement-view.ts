/**
 * Pure helpers for the frontend side of window placement: no DOM and no Tauri,
 * so the decisions can be unit tested once a frontend test runner exists. All
 * rectangles are in physical screen pixels, exactly as Rust's `place_window`
 * takes and returns them; only `physicalToCss` produces CSS pixels and only
 * `cssToPhysical` consumes them.
 */

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface Size {
  width: number;
  height: number;
}

export interface Point {
  x: number;
  y: number;
}

/** Converts physical pixels to CSS pixels with the window's scale factor. A
 * missing or nonsensical factor counts as 1 (no scaling). */
export function physicalToCss(px: number, scaleFactor: number): number {
  const scale = Number.isFinite(scaleFactor) && scaleFactor > 0 ? scaleFactor : 1;
  return px / scale;
}

/** Converts a size in CSS pixels to physical pixels with the window's scale
 * factor, rounded to whole pixels. The bar, panel and menu sizes are CSS
 * pixels (the page lays out in them); the window is sized in physical pixels,
 * so on a scale-2 Retina screen a 72x400 bar needs a 144x800 window. Mirrors
 * `placement::physical_size` in Rust. */
export function cssToPhysical(size: Size, scaleFactor: number): Size {
  const scale = Number.isFinite(scaleFactor) && scaleFactor > 0 ? scaleFactor : 1;
  return { width: Math.round(size.width * scale), height: Math.round(size.height * scale) };
}

/** The bar's screen rectangle from the live window position and the last
 * placement (where the bar sits inside the window, and how big it is). */
export function barFromWindow(windowPos: Point, last: { bar: Size; barOffset: Point }): Rect {
  return {
    x: windowPos.x + last.barOffset.x,
    y: windowPos.y + last.barOffset.y,
    width: last.bar.width,
    height: last.bar.height,
  };
}

/** Identity of a placement request, to skip one that would change nothing. */
export function requestKey(bar: Rect, size: Size, extra: Size): string {
  return [bar.x, bar.y, bar.width, bar.height, size.width, size.height, extra.width, extra.height].join(",");
}

/** True when the bar's top-left differs between two placements. */
export function barMoved(before: Rect | undefined, after: Rect): boolean {
  return !before || before.x !== after.x || before.y !== after.y;
}

/** CSS offset of the collapsed tab inside a window still `windowHeight` tall
 * (physical): the tab ends up centered on the bar's center, so it is painted
 * there before the window shrinks around it. */
export function collapsedTabDyCss(windowHeight: number, tabHeight: number, scaleFactor: number): number {
  return physicalToCss(Math.max(0, (windowHeight - tabHeight) / 2), scaleFactor);
}
