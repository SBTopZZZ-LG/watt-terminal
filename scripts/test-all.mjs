// Runs the CI checks on this machine: frontend (tsc, vitest, build) and Rust (clippy, cargo test).
// --docker also builds ci/linux/Dockerfile (Debian bookworm + zsh 5.9) and runs the Rust checks in it.
// --only=<id,...> runs only those steps. Step ids: tsc, test, build, clippy, cargo-test, docker-build, docker-test.
// Every step runs even after a failure. Exit code is 1 if any step failed. No dependencies.
import { spawn } from 'node:child_process';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const srcTauri = join(root, 'src-tauri');
const dockerContext = join(root, 'ci', 'linux');
const dockerImage = 'watt-ci-linux:bookworm';

const usage = 'Usage: node scripts/test-all.mjs [--docker] [--only=<step,...>]';
const args = process.argv.slice(2);
if (args.includes('--help') || args.includes('-h')) {
  console.log(`${usage}\n\nSteps: tsc, test, build, clippy, cargo-test, docker-build, docker-test (docker-* need --docker).`);
  process.exit(0);
}
const useDocker = args.includes('--docker');
const onlyArg = args.find((a) => a.startsWith('--only='));
const only = onlyArg ? onlyArg.slice('--only='.length).split(',').filter(Boolean) : null;
const unknownArgs = args.filter((a) => a !== '--docker' && !a.startsWith('--only='));
if (unknownArgs.length > 0) {
  console.error(`Unknown argument: ${unknownArgs[0]}\n${usage}`);
  process.exit(2);
}

// Inside the container: copy the repo (minus .git, node_modules and the host target dir) to /work, then run the
// Rust checks there. The target dir is the named volume at /target (CARGO_TARGET_DIR). dist/ comes from the host build.
const dockerRustScript = [
  'set -eu',
  'test -f /src/dist/index.html || { echo "dist/ missing: run npm run build first" >&2; exit 1; }',
  'tar -C /src --exclude=./.git --exclude=./node_modules --exclude=./src-tauri/target -cf - . | tar -C /work -xf -',
  // A Windows checkout with autocrlf has CRLF shim scripts, and zsh cannot parse them (the shim must be LF).
  "find /work/src-tauri/src/core/shim -type f -exec sed -i 's/\\r$//' {} +",
  'cd /work/src-tauri',
  'cargo clippy --all-targets -- -D warnings',
  'cargo test --no-fail-fast -j 3',
].join('\n');

const steps = [
  { id: 'tsc', label: 'TypeScript (npx tsc --noEmit)', cmd: 'npx', args: ['tsc', '--noEmit'], cwd: root },
  { id: 'test', label: 'Frontend tests (npm test)', cmd: 'npm', args: ['test'], cwd: root },
  { id: 'build', label: 'Frontend build (npm run build)', cmd: 'npm', args: ['run', 'build'], cwd: root },
  { id: 'clippy', label: 'Clippy on host', cmd: 'cargo', args: ['clippy', '--all-targets', '--', '-D', 'warnings'], cwd: srcTauri },
  { id: 'cargo-test', label: 'Cargo tests on host', cmd: 'cargo', args: ['test', '--no-fail-fast'], cwd: srcTauri },
];

const dockerSteps = [
  {
    id: 'docker-build',
    label: 'Docker image build',
    cmd: 'docker',
    args: ['build', '-t', dockerImage, dockerContext],
    cwd: root,
  },
  {
    id: 'docker-test',
    label: 'Docker: clippy + cargo test (bookworm, zsh 5.9)',
    cmd: 'docker',
    args: [
      'run', '--rm',
      '--mount', `type=bind,source=${root},target=/src,readonly`,
      '--mount', 'type=volume,source=watt-ci-cargo,target=/usr/local/cargo/registry',
      '--mount', 'type=volume,source=watt-ci-target,target=/target',
      dockerImage, 'bash', '-c', dockerRustScript,
    ],
    cwd: root,
    requires: 'docker-build',
  },
];

const enabled = useDocker ? [...steps, ...dockerSteps] : steps;
const enabledIds = new Set(enabled.map((s) => s.id));
if (only) {
  const unknown = only.filter((id) => !enabledIds.has(id));
  if (unknown.length > 0) {
    console.error(`Unknown or disabled step(s) for this run: ${unknown.join(', ')}\nEnabled: ${[...enabledIds].join(', ')}`);
    process.exit(2);
  }
}
const selected = only ? enabled.filter((s) => only.includes(s.id)) : enabled;

// npm and npx are .cmd shims on Windows, so they run through a shell there (command joined into one string, no
// user paths in those args). Everything else runs directly with an args array.
function runStep(step) {
  const viaShell = process.platform === 'win32' && (step.cmd === 'npm' || step.cmd === 'npx');
  const command = viaShell ? [step.cmd, ...step.args].join(' ') : step.cmd;
  const argv = viaShell ? [] : step.args;
  return new Promise((done) => {
    const child = spawn(command, argv, { cwd: step.cwd, stdio: 'inherit', shell: viaShell });
    child.on('error', (err) => {
      console.error(`Could not start ${step.cmd}: ${err.message}`);
      done(1);
    });
    child.on('exit', (code) => done(code ?? 1));
  });
}

const results = [];
const failedIds = new Set();
for (const step of selected) {
  if (step.requires && failedIds.has(step.requires)) {
    results.push({ label: step.label, status: 'SKIP', seconds: 0 });
    console.log(`\n==> ${step.label}: skipped, ${step.requires} failed`);
    continue;
  }
  console.log(`\n==> ${step.label}`);
  const started = Date.now();
  const code = await runStep(step);
  const seconds = Math.round((Date.now() - started) / 1000);
  if (code === 0) {
    results.push({ label: step.label, status: 'PASS', seconds });
  } else {
    failedIds.add(step.id);
    results.push({ label: step.label, status: 'FAIL', seconds });
  }
}

const width = Math.max(...results.map((r) => r.label.length), 4);
console.log('\n== Summary ==');
console.log(`${'step'.padEnd(width)}  result  time`);
for (const r of results) {
  console.log(`${r.label.padEnd(width)}  ${r.status.padEnd(6)}  ${r.seconds}s`);
}
const count = (status) => results.filter((r) => r.status === status).length;
console.log(`\nPassed ${count('PASS')}, failed ${count('FAIL')}, skipped ${count('SKIP')}`);
process.exit(count('FAIL') > 0 ? 1 : 0);
