import type { GridInfo } from './tab';

/**
 * The grid chip's tooltip: the numbers to report when the text looks clipped, one per line (the tooltip keeps line breaks).
 */
export function gridTip(info: GridInfo): string {
  return [
    `xterm cols ${info.cols}, rows ${info.rows}`,
    `host ${info.hostWidth}px, grid ${info.screenWidth}px`,
    `devicePixelRatio ${info.devicePixelRatio}`,
  ].join('\n');
}
