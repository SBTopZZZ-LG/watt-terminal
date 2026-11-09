import type { Cwd } from './types';

/** Replaces a leading macOS or Linux home directory with `~`. */
export function shortenPath(path: string): string {
  const home = /^\/(?:Users|home)\/[^/]+(?=\/|$)/.exec(path);
  if (home) {
    return `~${path.slice(home[0].length)}`;
  }
  return path;
}

export function formatCwd(cwd: Cwd | null): string {
  if (cwd === null) {
    return 'unknown';
  }
  return `${cwd.host}:${shortenPath(cwd.path)}`;
}

/** Pretty JSON for a tool result, cut to `limit` characters so a huge output cannot flood the transcript. */
export function previewJson(value: unknown, limit = 4000): string {
  let text: string;
  try {
    text = JSON.stringify(value, null, 2) ?? 'null';
  } catch {
    text = String(value);
  }
  return text.length > limit ? `${text.slice(0, limit)}\n... (cut)` : text;
}
