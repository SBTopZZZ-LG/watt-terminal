import type { ReactNode } from 'react';

// The raw text of a rendered fenced code block, for its copy button. Walks the React children, so the copy is what the
// model wrote, not any markup a highlighter put around it. Pure, so it is tested without the DOM.

/** The text inside a rendered node: strings and numbers as they are, arrays and element children recursively. */
export function nodeText(children: ReactNode): string {
  return dropFinalNewline(collect(children));
}

function collect(node: ReactNode): string {
  if (typeof node === 'string' || typeof node === 'number') {
    return String(node);
  }
  if (Array.isArray(node)) {
    return node.map(collect).join('');
  }
  if (node && typeof node === 'object' && 'props' in node) {
    return collect((node as { props: { children?: ReactNode } }).props.children);
  }
  return '';
}

/** One trailing line break is the fence's closing newline, not part of the code. */
function dropFinalNewline(text: string): string {
  if (text.endsWith('\r\n')) {
    return text.slice(0, -2);
  }
  return text.endsWith('\n') ? text.slice(0, -1) : text;
}
