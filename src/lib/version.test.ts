import { describe, expect, it } from 'vitest';
// @ts-expect-error type error without @types/node package
import { readFileSync } from 'node:fs';
import pkg from '../../package.json';
import config from '../../src-tauri/tauri.conf.json';

// The app version lives in three manifests. Settings > About shows package.json's; the bundle and the Rust crate carry the
// other two. They must be bumped together.

describe('the app version is the same in every manifest', () => {
  it('package.json, src-tauri/tauri.conf.json and src-tauri/Cargo.toml agree', () => {
    const cargo = readFileSync(new URL('../../src-tauri/Cargo.toml', import.meta.url), 'utf8') as string;
    // The first `version = "..."` line after the [package] table header.
    const cargoVersion = cargo.match(/^\[package\][\s\S]*?^version = "([^"]+)"/m)?.[1];
    expect(cargoVersion).toBeDefined();
    expect(config.version).toBe(pkg.version);
    expect(cargoVersion).toBe(pkg.version);
  });
});
