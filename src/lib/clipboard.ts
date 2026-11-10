// Copies text to the clipboard. The webview may refuse navigator.clipboard (WKWebView in Tauri does, unless the
// call is made from a user gesture and the page is secure), so there is a fallback through a hidden textarea.
// The clipboard and the document are passed in, so the logic runs without a browser.

export interface ClipboardEnv {
  /** navigator.clipboard, or nothing where the webview does not provide it. */
  clipboard?: { writeText(text: string): Promise<void> } | null;
  /** The document, for the fallback. */
  document?: CopyDocument | null;
}

/** The part of the document the fallback uses. */
export type CopyDocument = Pick<Document, 'createElement' | 'body' | 'execCommand'>;

/**
 * Resolves true when the text was copied. Tries navigator.clipboard.writeText first; if that is unavailable or throws,
 * copies through a hidden, selected textarea with document.execCommand('copy'). Resolves false only when both fail.
 */
export async function copyText(text: string, env: ClipboardEnv): Promise<boolean> {
  if (env.clipboard) {
    try {
      await env.clipboard.writeText(text);
      return true;
    } catch {
      // Fall through to the textarea.
    }
  }
  return copyWithTextarea(text, env.document);
}

function copyWithTextarea(text: string, doc: CopyDocument | null | undefined): boolean {
  if (!doc) {
    return false;
  }
  try {
    const area = doc.createElement('textarea');
    area.value = text;
    area.setAttribute('readonly', '');
    // Off screen and invisible, so the focus and selection do not show.
    area.style.position = 'fixed';
    area.style.left = '-9999px';
    area.style.top = '0';
    area.style.opacity = '0';
    doc.body.appendChild(area);
    try {
      area.select();
      return doc.execCommand('copy');
    } finally {
      doc.body.removeChild(area);
    }
  } catch {
    return false;
  }
}
