import { memo } from 'react';
import ReactMarkdown from 'react-markdown';
import type { Components, ExtraProps } from 'react-markdown';
import remarkGfm from 'remark-gfm';

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

const components: Components = {
  // Keep the link text. The URL is on hover, and nothing navigates.
  a: ({ href, children }) => <span className="md-link" title={href}>{children}</span>,
  // Never fetch an image. Show the alt text as plain text, and nothing when there is none.
  img: ({ alt }) => (alt ? <span className="md-img">{alt}</span> : null),
  pre: ({ node, ...props }) => <pre {...props} data-lang={fenceLanguage(node)} />,
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
