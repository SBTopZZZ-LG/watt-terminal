import { memo, useEffect, useRef, useState } from 'react';
import type { ComponentProps } from 'react';
import ReactMarkdown from 'react-markdown';
import type { Components, ExtraProps } from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { copyText } from '../lib/clipboard';
import { nodeText } from '../lib/codeText';

/*
 * Agent replies, rendered as markdown.
 *
 * The text comes from a model, so it is untrusted. Raw HTML is never rendered: react-markdown turns it into escaped
 * text, and rehype-raw is not used. Links never navigate the webview: they render as spans. Images are never loaded:
 * they render as their alt text. Keep those rules when adding overrides.
 */

const remarkPlugins = [remarkGfm];

function classNames(node: ExtraProps['node']): string[] {
  const value = node?.properties.className;
  return Array.isArray(value) ? value.map(String) : [];
}

/** The language from the fence info (```sh gives 'sh'). Unlabelled fences show as 'text'. */
function fenceLanguage(node: ExtraProps['node']): string {
  const code = node?.children.find((child) => child.type === 'element');
  if (code?.type !== 'element') {
    return 'text';
  }
  const lang = classNames(code)
    .find((name) => name.startsWith('language-'))
    ?.slice('language-'.length);
  return lang ? lang.slice(0, 32) : 'text';
}

/** A list with at least one GFM task item (`- [ ] todo`). */
function isTaskList(node: ExtraProps['node']): boolean {
  return node?.children.some((child) => child.type === 'element' && classNames(child).includes('task-list-item')) ?? false;
}

function isDoneTask(node: ExtraProps['node']): boolean {
  if (!classNames(node).includes('task-list-item')) {
    return false;
  }
  return node?.children.some((child) => child.type === 'element' && child.tagName === 'input' && Boolean(child.properties.checked)) ?? false;
}

/** How long the copy result stays on screen. */
const FEEDBACK_MS = 1500;

/**
 * A fenced block: the pre, and a copy button pinned to its top-right corner. The button sits beside the pre, not inside
 * it, so it stays put when a wide line scrolls. It copies the raw text (see codeText.ts).
 */
function CodeBlock({ lang, children, ...pre }: ComponentProps<'pre'> & { lang: string }) {
  return (
    <div className="md-code">
      <pre {...pre} data-lang={lang}>
        {children}
      </pre>
      <CopyCodeButton text={nodeText(children)} />
    </div>
  );
}

type CopyState = 'idle' | 'copied' | 'failed';

function CopyCodeButton({ text }: { text: string }) {
  const [state, setState] = useState<CopyState>('idle');
  const timer = useRef<number | undefined>(undefined);

  useEffect(() => () => window.clearTimeout(timer.current), []);

  const onClick = async () => {
    const ok = await copyText(text, { clipboard: navigator.clipboard, document });
    setState(ok ? 'copied' : 'failed');
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setState('idle'), FEEDBACK_MS);
  };

  return (
    <div className={`md-code-copy${state === 'idle' ? '' : ' shown'}`}>
      {/* Its own live region, outside the button: a button's children are not announced. */}
      <span className={`md-code-status${state === 'failed' ? ' failed' : ''}`} role="status" aria-live="polite">
        {state === 'copied' ? 'Copied' : state === 'failed' ? 'Copy failed' : ''}
      </span>
      {/* Mouse-down keeps focus where it was, so the prompt keeps the focus it has (see inputFocus.ts). */}
      <button type="button" className="md-code-button" aria-label="Copy code" onMouseDown={(e) => e.preventDefault()} onClick={onClick}>
        <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" strokeWidth="1.25" aria-hidden="true">
          <rect x="5.5" y="5.5" width="8" height="8" rx="1" />
          <path d="M10.5 5.5V3.5a1 1 0 0 0-1-1h-6a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2" />
        </svg>
      </button>
    </div>
  );
}

const components: Components = {
  // Keep the link text. The URL is on hover, and nothing navigates.
  a: ({ href, children }) => <span className="md-link" title={href}>{children}</span>,
  // Never fetch an image. Show the alt text as plain text, and nothing when there is none.
  img: ({ alt }) => (alt ? <span className="md-img">{alt}</span> : null),
  pre: ({ node, ...props }) => <CodeBlock {...props} lang={fenceLanguage(node)} />,
  // Wide tables scroll inside the panel instead of stretching it.
  table: ({ node, ...props }) => (
    <div className="md-table">
      <table {...props} />
    </div>
  ),
  ul: ({ node, ...props }) => <ul {...props} className={isTaskList(node) ? 'tasks' : undefined} />,
  li: ({ node, ...props }) => <li {...props} className={isDoneTask(node) ? 'done' : undefined} />,
  // GFM's task checkbox is shown (the CSS draws the box), but it is disabled and read-only.
  input: ({ type, checked }) => (type === 'checkbox' ? <input type="checkbox" checked={Boolean(checked)} disabled readOnly /> : null),
};

function MarkdownText({ text }: { text: string }) {
  return (
    <div className="md">
      <ReactMarkdown remarkPlugins={remarkPlugins} components={components}>
        {text}
      </ReactMarkdown>
    </div>
  );
}

/** Memoised, so a message that has not changed is not reparsed while a later message streams. */
export const Markdown = memo(MarkdownText);
