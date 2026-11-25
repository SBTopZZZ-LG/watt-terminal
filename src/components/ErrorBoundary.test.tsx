import { describe, expect, it, vi } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { CrashView, crashDetails, ErrorBoundary, installCrashLogging } from './ErrorBoundary';

describe('ErrorBoundary', () => {
  it('renders its children when nothing throws', () => {
    const html = renderToStaticMarkup(
      <ErrorBoundary>
        <p>Still here</p>
      </ErrorBoundary>,
    );
    expect(html).toBe('<p>Still here</p>');
  });

  it('keeps the thrown error in state', () => {
    const error = new Error('boom');
    expect(ErrorBoundary.getDerivedStateFromError(error)).toEqual({ error });
  });

  it('turns a thrown non-Error into an Error', () => {
    const state = ErrorBoundary.getDerivedStateFromError('plain text');
    expect(state.error).toBeInstanceOf(Error);
    expect(state.error?.message).toBe('plain text');
  });
});

describe('CrashView', () => {
  it('shows the message, the stack, the component stack and both buttons', () => {
    const error = new Error('Cannot read the screen');
    error.stack = 'Error: Cannot read the screen\n    at render (app.js:1:2)';
    const html = renderToStaticMarkup(<CrashView error={error} componentStack={'\n    in TerminalView'} />);
    expect(html).toContain('hit an error and stopped');
    expect(html).toContain('<p class="crash-text">Cannot read the screen</p>');
    expect(html).toContain('<pre class="crash-details">');
    expect(html).toContain('at render (app.js:1:2)');
    expect(html).toContain('Component stack:');
    expect(html).toContain('in TerminalView');
    expect(html).toContain('Copy details');
    expect(html).toContain('Reload interface');
  });
});

describe('crashDetails', () => {
  it('is the stack alone when there is no component stack', () => {
    const error = new Error('x');
    error.stack = 'Error: x\n    at f (a.js:1:1)';
    expect(crashDetails(error, null)).toBe('Error: x\n    at f (a.js:1:1)');
  });

  it('falls back to name and message when the error has no stack', () => {
    const error = new RangeError('out of range');
    error.stack = undefined;
    expect(crashDetails(error, null)).toBe('RangeError: out of range');
  });
});

describe('installCrashLogging', () => {
  it('logs uncaught errors and unhandled rejections to the console', () => {
    const handlers = new Map<string, (event: Event) => void>();
    const target = {
      addEventListener: (type: string, listener: (event: Event) => void) => {
        handlers.set(type, listener);
      },
    };
    const log = vi.spyOn(console, 'error').mockImplementation(() => undefined);
    try {
      installCrashLogging(target);
      handlers.get('error')?.({ error: new Error('uncaught') } as unknown as Event);
      handlers.get('unhandledrejection')?.({ reason: 'rejected' } as unknown as Event);
      expect(log).toHaveBeenCalledTimes(2);
      expect(log.mock.calls[1][1]).toBe('rejected');
    } finally {
      log.mockRestore();
    }
  });
});
