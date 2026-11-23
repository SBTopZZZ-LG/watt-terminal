import { describe, expect, it } from 'vitest';
// @ts-expect-error type error without @types/node package
import { readdirSync, readFileSync, statSync } from 'node:fs';

/*
 * The drift guard for the built-in `watt` skill. Its markdown (src-tauri/src/core/skills/builtin) tells the agent UI text, and
 * every label it quotes is in curly double quotes. Each quoted text must occur verbatim in the app's own source (non-test
 * .ts and .tsx files under src/). When a label is renamed in the app, this test fails until the cheatsheet is updated.
 */

const BUILTIN_DIR = new URL('../../src-tauri/src/core/skills/builtin/', import.meta.url);
const SRC_DIR = new URL('../', import.meta.url);
const QUOTED = /“([^”\n]+)”/g;

/** The text of every non-test .ts and .tsx file under a folder, recursively, joined. */
function sourceText(dir: URL): string {
  return readdirSync(dir)
    .map((name: string) => {
      const url = new URL(name, dir);
      if (statSync(url).isDirectory()) {
        return sourceText(new URL(`${name}/`, dir));
      }
      const isSource = /\.(ts|tsx)$/.test(name) && !/\.test\.(ts|tsx)$/.test(name);
      return isSource ? readFileSync(url, 'utf8') : '';
    })
    .join('\n');
}

function cheatsheetFiles(): { name: string; text: string }[] {
  return readdirSync(BUILTIN_DIR)
    .filter((name: string) => name.endsWith('.md'))
    .map((name: string) => ({ name, text: readFileSync(new URL(name, BUILTIN_DIR), 'utf8') as string }));
}

describe('built-in watt skill: drift guard', () => {
  it('every quoted UI label occurs verbatim in the app source', () => {
    const source = sourceText(SRC_DIR);
    const missing: string[] = [];
    for (const file of cheatsheetFiles()) {
      for (const match of file.text.matchAll(QUOTED)) {
        if (!source.includes(match[1])) {
          missing.push(`${file.name}: “${match[1]}”`);
        }
      }
    }
    expect(missing).toEqual([]);
  });

  it('the cheatsheet quotes a real set of labels, so the check is not empty', () => {
    const count = cheatsheetFiles().reduce((n, file) => n + [...file.text.matchAll(QUOTED)].length, 0);
    expect(count).toBeGreaterThan(60);
  });

  it('the skill has its instructions and its two reference files', () => {
    const names = cheatsheetFiles().map((file) => file.name).sort();
    expect(names).toEqual(['SKILL.md', 'troubleshooting.md', 'ui.md']);
  });
});
