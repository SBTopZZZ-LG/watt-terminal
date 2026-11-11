import { describe, expect, it } from 'vitest';
import config from '../../src-tauri/tauri.conf.json';

// The Content-Security-Policy that Tauri ships in a built app (app.security.csp). The policy is parsed into directives
// so each rule can be checked on its own. The browser check in /tmp/poc-ui-2/csp-check.mjs reads the same string.

const security = config.app.security as { csp?: unknown; devCsp?: unknown; dangerousDisableAssetCspModification?: unknown };

function parsePolicy(policy: string): Map<string, string[]> {
  const directives = new Map<string, string[]>();
  for (const part of policy.split(';')) {
    const [name, ...sources] = part.trim().split(/\s+/);
    if (name) {
      directives.set(name, sources);
    }
  }
  return directives;
}

const csp = security.csp;
const directives = typeof csp === 'string' ? parsePolicy(csp) : new Map<string, string[]>();

describe('shipped Content-Security-Policy (src-tauri/tauri.conf.json)', () => {
  it('is set as a policy string, not left null', () => {
    expect(typeof csp).toBe('string');
    expect((csp as string).trim().length).toBeGreaterThan(0);
  });

  it('allows scripts from the app itself only', () => {
    expect(directives.get('script-src')).toEqual(["'self'"]);
  });

  it('never allows eval or other dynamic code paths', () => {
    expect(csp as string).not.toMatch(/unsafe-eval|wasm-unsafe-eval|unsafe-hashes/);
  });

  it('defaults every fetch type to the app itself', () => {
    expect(directives.get('default-src')).toEqual(["'self'"]);
  });

  it('blocks plugins and framing', () => {
    expect(directives.get('object-src')).toEqual(["'none'"]);
    expect(directives.get('frame-src')).toEqual(["'none'"]);
  });

  it('allows only the Tauri IPC origins for connections, and no other network host', () => {
    expect(directives.get('connect-src')).toEqual(['ipc:', 'http://ipc.localhost']);
    const hosts = [...directives.values()].flat().filter((source) => /^(https?|wss?):|^\*$|^[a-z0-9.-]+\.[a-z]{2,}/i.test(source));
    expect(hosts.filter((source) => source !== 'http://ipc.localhost')).toEqual([]);
  });

  it('keeps styles limited to the app, with inline styles allowed because xterm injects <style> elements', () => {
    expect(directives.get('style-src')).toEqual(["'self'", "'unsafe-inline'"]);
  });

  it('lets fonts and images come from the app itself (images also allow data: URIs)', () => {
    expect(directives.get('font-src')).toEqual(["'self'"]);
    expect(directives.get('img-src')).toEqual(["'self'", 'data:']);
  });

  it('keeps the policy closed for form posts and base URLs', () => {
    expect(directives.get('base-uri')).toEqual(["'none'"]);
    expect(directives.get('form-action')).toEqual(["'none'"]);
  });

  it('has no development-only policy that could be mistaken for the shipped one', () => {
    expect(security.devCsp).toBeUndefined();
  });

  it('stops Tauri from adding nonces or hashes to style-src, which would silently disable unsafe-inline', () => {
    expect(security.dangerousDisableAssetCspModification).toEqual(['style-src']);
  });
});
