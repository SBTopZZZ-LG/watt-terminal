import { Component, useState, type ErrorInfo, type ReactNode } from 'react';
import { copyText } from '../lib/clipboard';

// Catches an error thrown while rendering, or in a lifecycle, below it. Without a boundary React unmounts the whole tree and
// the window goes blank with nothing logged. The fallback shows what failed and offers a copy and a reload. Nothing is sent
// anywhere and nothing is written to disk.

interface ErrorBoundaryProps {
  children: ReactNode;
}

interface ErrorBoundaryState {
  error: Error | null;
  componentStack: string | null;
}

export class ErrorBoundary extends Component<ErrorBoundaryProps, ErrorBoundaryState> {
  state: ErrorBoundaryState = { error: null, componentStack: null };

  static getDerivedStateFromError(error: unknown): Partial<ErrorBoundaryState> {
    return { error: asError(error) };
  }

  componentDidCatch(error: unknown, info: ErrorInfo): void {
    console.error('Watt interface error:', error, info.componentStack);
    this.setState({ error: asError(error), componentStack: info.componentStack ?? null });
  }

  render(): ReactNode {
    if (this.state.error !== null) {
      return <CrashView error={this.state.error} componentStack={this.state.componentStack} />;
    }
    return this.props.children;
  }
}

function asError(value: unknown): Error {
  return value instanceof Error ? value : new Error(String(value));
}

/** The text the copy button takes: the stack (or name and message) and the component stack. Nothing else. */
export function crashDetails(error: Error, componentStack: string | null): string {
  const parts = [error.stack ?? `${error.name}: ${error.message}`];
  if (componentStack) {
    parts.push(`Component stack:${componentStack}`);
  }
  return parts.join('\n\n');
}

/** The fallback view. It uses the app's theme tokens and stands on its own: no layout from the rest of the app. */
export function CrashView({ error, componentStack }: { error: Error; componentStack: string | null }) {
  const [copied, setCopied] = useState<'idle' | 'copied' | 'failed'>('idle');
  const details = crashDetails(error, componentStack);
  const onCopy = () => {
    void copyText(details, { clipboard: navigator.clipboard, document }).then((ok) =>
      setCopied(ok ? 'copied' : 'failed'),
    );
  };
  return (
    <div className="crash-view" role="alert">
      <div className="crash-card">
        <h1 className="crash-title">Watt's interface hit an error and stopped.</h1>
        <p className="crash-text">{error.message}</p>
        <pre className="crash-details">{details}</pre>
        <div className="crash-actions">
          <button type="button" className="crash-button" onClick={onCopy}>
            Copy details
          </button>
          <button type="button" className="crash-button" onClick={() => window.location.reload()}>
            Reload interface
          </button>
          {copied !== 'idle' && (
            <span className="crash-text" role="status">
              {copied === 'copied' ? 'Copied.' : 'Copy failed.'}
            </span>
          )}
        </div>
      </div>
    </div>
  );
}

/**
 * Logs uncaught errors and unhandled promise rejections. They do not unmount React, so they show no UI: the console is the
 * only record, as before, with the same fields available to a developer.
 */
export function installCrashLogging(target: {
  addEventListener(type: string, listener: (event: Event) => void): void;
}): void {
  target.addEventListener('error', (event) => {
    const e = event as ErrorEvent;
    console.error('Watt uncaught error:', e.error ?? e.message);
  });
  target.addEventListener('unhandledrejection', (event) => {
    console.error('Watt unhandled rejection:', (event as PromiseRejectionEvent).reason);
  });
}
