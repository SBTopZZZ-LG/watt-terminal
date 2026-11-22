// Dependency-free MCP stdio server for the runtime tests (src-tauri/tests/mcp_runtime.rs). Newline-delimited JSON-RPC 2.0
// on stdin and stdout. Usage: node server.cjs [mode] [pid-file]
//   (none)               the normal server
//   stubborn <pid-file>  also starts a grandchild that ignores stdin EOF, and writes the grandchild's pid to <pid-file>;
//                        the root ignores stdin EOF too, so only a tree kill ends it
//   exit-after-tools     exits right after it answers tools/list
// The secret the tests put in FIXTURE_SECRET is echoed to stderr at start and returned by the `leak` tool.

const fs = require('node:fs');
const { spawn } = require('node:child_process');

const mode = process.argv[2] || '';
const pidFile = process.argv[3];

const tools = [
  { name: 'echo', description: 'Echo the message.', inputSchema: { type: 'object', properties: { message: { type: 'string' } } } },
  { name: 'pid', description: 'Report this process id.', inputSchema: { type: 'object' } },
  { name: 'slow', description: 'Answer after ms milliseconds.', inputSchema: { type: 'object', properties: { ms: { type: 'number' } } } },
  { name: 'fail', description: 'Report a tool failure.', inputSchema: { type: 'object' } },
  { name: 'big', description: 'Return 100 KB of text.', inputSchema: { type: 'object' } },
  { name: 'image', description: 'Return a small image.', inputSchema: { type: 'object' } },
  { name: 'leak', description: 'Return the secret environment value.', inputSchema: { type: 'object' } },
  { name: 'exit', description: 'Exit the process without answering.', inputSchema: { type: 'object' } },
  { name: 'add-tool', description: 'Add a tool and announce the list change.', inputSchema: { type: 'object' } },
  {
    name: 'weird name ü',
    description: 'A tool whose name has spaces and unicode, with a $defs schema.',
    inputSchema: {
      $schema: 'https://json-schema.org/draft/2020-12/schema',
      type: 'object',
      properties: { item: { $ref: '#/$defs/Item' } },
      $defs: { Item: { type: 'object', properties: { id: { type: 'integer', format: 'uint32', minimum: 0 } }, required: ['id'] } },
    },
  },
];

process.stderr.write(`fixture started FIXTURE_SECRET=${process.env.FIXTURE_SECRET ?? 'unset'}\n`);

if (mode === 'stubborn') {
  const grandchild = spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], { stdio: 'ignore' });
  fs.writeFileSync(pidFile, String(grandchild.pid));
  // Keeps the root alive after its stdin ends, so only a kill ends it.
  setInterval(() => {}, 1000);
}

const pending = new Map();

function send(message) {
  process.stdout.write(`${JSON.stringify(message)}\n`);
}

function result(id, value) {
  send({ jsonrpc: '2.0', id, result: value });
}

function text(id, body, extra = {}) {
  result(id, { content: [{ type: 'text', text: body }], ...extra });
}

function callTool(id, params) {
  const args = params.arguments || {};
  switch (params.name) {
    case 'echo':
      return text(id, `echo: ${args.message}`);
    case 'pid':
      return text(id, String(process.pid));
    case 'slow': {
      const timer = setTimeout(() => {
        pending.delete(id);
        text(id, 'slow done');
      }, Number(args.ms) || 0);
      pending.set(id, timer);
      return undefined;
    }
    case 'fail':
      return result(id, { content: [{ type: 'text', text: 'boom from fixture' }], isError: true });
    case 'big':
      return text(id, 'x'.repeat(100 * 1024));
    case 'image':
      return result(id, { content: [{ type: 'image', data: 'AAAA', mimeType: 'image/png' }] });
    case 'leak':
      return text(id, `secret is ${process.env.FIXTURE_SECRET ?? 'unset'}`);
    case 'exit':
      process.exit(1);
      return undefined;
    case 'add-tool':
      // Adds a tool and announces the change, as a server does when its tool list changes.
      tools.push({ name: 'added', description: 'Added after connect.', inputSchema: { type: 'object' } });
      send({ jsonrpc: '2.0', method: 'notifications/tools/list_changed' });
      return text(id, 'added');
    case 'weird name ü':
      return text(id, `weird ok ${JSON.stringify(args)}`);
    default:
      return send({ jsonrpc: '2.0', id, error: { code: -32602, message: `unknown tool ${params.name}` } });
  }
}

function handle(message) {
  const { id, method, params } = message;
  switch (method) {
    case 'initialize':
      return result(id, {
        protocolVersion: params.protocolVersion,
        capabilities: { tools: { listChanged: true } },
        serverInfo: { name: 'watt-fixture', version: '1.0.0' },
      });
    case 'notifications/initialized':
      return undefined;
    case 'notifications/cancelled': {
      const timer = pending.get(params.requestId);
      if (timer) {
        clearTimeout(timer);
        pending.delete(params.requestId);
      }
      return undefined;
    }
    case 'tools/list': {
      if (mode === 'exit-after-tools') {
        process.stdout.write(`${JSON.stringify({ jsonrpc: '2.0', id, result: { tools } })}\n`, () => process.exit(0));
        return undefined;
      }
      return result(id, { tools });
    }
    case 'tools/call':
      return callTool(id, params);
    case 'ping':
      return result(id, {});
    default:
      if (id !== undefined) {
        send({ jsonrpc: '2.0', id, error: { code: -32601, message: `method not found: ${method}` } });
      }
      return undefined;
  }
}

let buffer = '';
process.stdin.setEncoding('utf8');
process.stdin.on('data', (chunk) => {
  buffer += chunk;
  let newline;
  while ((newline = buffer.indexOf('\n')) >= 0) {
    const line = buffer.slice(0, newline).trim();
    buffer = buffer.slice(newline + 1);
    if (line) handle(JSON.parse(line));
  }
});
process.stdin.on('end', () => {
  if (mode !== 'stubborn') process.exit(0);
});
