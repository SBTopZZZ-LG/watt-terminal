import { describe, expect, it } from 'vitest';
// @ts-expect-error type error without @types/node package
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { findHookOrderProblems } from './hooksOrder';

/*
 * The regression guard for the rules of hooks (hooksOrder.ts). A hook after an early return, or inside a conditional, makes React
 * throw "Rendered fewer hooks than expected" once the same component rerenders on another path (InputBar on the alternate screen).
 * The source check reads every non-test .ts and .tsx file under src/; the fixtures pin what counts as a violation.
 */

const SRC_DIR = new URL('../', import.meta.url);

/** The non-test .ts and .tsx files under a folder, recursively, with their paths relative to it. */
function sourceFiles(dir: URL, prefix = ''): { name: string; text: string }[] {
  return readdirSync(dir).flatMap((entry: string): { name: string; text: string }[] => {
    const url = new URL(entry, dir);
    if (statSync(url).isDirectory()) {
      return sourceFiles(new URL(`${entry}/`, dir), `${prefix}${entry}/`);
    }
    const isSource = /\.(ts|tsx)$/.test(entry) && !/\.test\.(ts|tsx)$/.test(entry);
    return isSource ? [{ name: `${prefix}${entry}`, text: readFileSync(url, 'utf8') as string }] : [];
  });
}

/** Source lines joined, so each fixture's line numbers are the ones written here. */
const src = (...lines: string[]) => lines.join('\n');

/** The problems in a fixture, as `line owner hook reason`. */
const problems = (text: string) => findHookOrderProblems('Fixture.tsx', text).map((p) => `${p.line} ${p.owner} ${p.hook} ${p.reason}`);

describe('rules of hooks in the app source', () => {
  it('every component and hook under src/ calls its hooks in a fixed order', () => {
    const found = sourceFiles(SRC_DIR).flatMap((f) =>
      findHookOrderProblems(f.name, f.text).map((p) => `${f.name}:${p.line} ${p.owner} ${p.hook} ${p.reason}`),
    );
    expect(found).toEqual([]);
  });
});

describe('findHookOrderProblems', () => {
  it('passes hooks that all run before an early return', () => {
    expect(
      problems(
        src(
          'function Bar(props: Props) {',
          '  const [a] = useState(0);',
          "  useEffect(() => {}, []);",
          "  if (props.mode === 'exited') {",
          '    return null;',
          '  }',
          '  return a;',
          '}',
        ),
      ),
    ).toEqual([]);
  });

  it('flags a hook after an early return, the InputBar shape', () => {
    expect(
      problems(
        src(
          'function Bar(props: Props) {',
          "  if (props.mode === 'exited') {",
          '    return null;',
          '  }',
          '  const ref = React.useRef(null);',
          '  return ref;',
          '}',
        ),
      ),
    ).toEqual(['5 Bar React.useRef after-return']);
  });

  it('flags a hook inside an if, a loop, a try, or a logical, ternary or switch branch', () => {
    expect(
      problems(
        src(
          'function Bar(props: Props) {',
          '  if (props.ok) {',
          '    useEffect(() => {});',
          '  }',
          '  const v = props.ok && useMemo(() => 1, []);',
          '  const w = props.ok ? useRef(0) : null;',
          '  for (const x of props.items) useState(x);',
          '  try { useRef(1); } catch { /* ignored */ }',
          '  switch (props.mode) { case 1: useId(); }',
          '  return v ?? w;',
          '}',
        ),
      ),
    ).toEqual([
      '3 Bar useEffect conditional',
      '5 Bar useMemo conditional',
      '6 Bar useRef conditional',
      '7 Bar useState conditional',
      '8 Bar useRef conditional',
      '9 Bar useId conditional',
    ]);
  });

  it('allows hooks in a nested callback, a hook in an if condition, and React 19 use() after a return', () => {
    expect(
      problems(
        src(
          'function Bar(props: Props) {',
          '  const [items] = useState([]);',
          '  useEffect(() => {',
          '    if (props.ok) useRef(1);',
          '  });',
          "  if (props.mode === 'x') return null;",
          '  const value = use(props.promise);',
          '  return items ?? value;',
          '}',
        ),
      ),
    ).toEqual([]);
  });

  it('checks components wrapped in memo or forwardRef, and named by the variable', () => {
    expect(
      problems(
        src(
          'export const Bar = memo((props: Props) => {',
          '  if (props.ok) return null;',
          '  const ref = useRef(null);',
          '  return ref;',
          '});',
          'export const Fw = forwardRef<HTMLDivElement, Props>((props, ref) => {',
          '  if (props.ok) useRef(1);',
          '  return null;',
          '});',
        ),
      ),
    ).toEqual(['3 Bar useRef after-return', '7 Fw useRef conditional']);
  });
});
