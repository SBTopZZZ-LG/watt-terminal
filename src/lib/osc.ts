// Parses OSC 133/633 payloads (the text after "133;" or "633;"). The shell tags every event with its nonce as
// field 2 (`n=<nonce>`). Rust is the authority for records and gating. The frontend uses these events only to
// place markers and gutter decorations, so a payload without an accepted nonce is ignored, never acted on.
//
// The accepted nonces are the local shell's and one per nested level (see session.ts). A nested level's nonce is added
// when Rust announces the level, and removed when the level is popped.
//
// Payload shapes (from src-tauri/src/core/shim/integration.zsh and scanner.rs):
//   A;n=N                  prompt start
//   P;n=N;k=s              continuation prompt (PS2)
//   C;n=N                  command start (after Enter)
//   D;n=N;exit=E;ps=0,3    command end
//   Hello;n=N;v=1;...      633 only: integration loaded

export type ShellEvent =
  | { kind: 'prompt_start' }
  | { kind: 'continuation' }
  | { kind: 'command_start' }
  | { kind: 'command_end'; exit: number; pipestatus: number[] }
  | { kind: 'hello' };

/**
 * Returns the event, or null when the payload is not from one of the accepted shells: no nonce yet, a `n=` that is not in
 * the set, or a missing one, or an event kind this UI does not use. Each nonce is compared exactly and is never logged.
 */
export function parseShellPayload(ident: 133 | 633, payload: string, nonces: ReadonlySet<string>): ShellEvent | null {
  const fields = payload.split(';');
  if (fields.length < 2 || !fields[1].startsWith('n=')) {
    return null;
  }
  const nonce = fields[1].slice(2);
  if (nonce === '' || !nonces.has(nonce)) {
    return null;
  }
  const kind = fields[0];
  if (ident === 633) {
    return kind === 'Hello' ? { kind: 'hello' } : null;
  }
  switch (kind) {
    case 'A':
      return { kind: 'prompt_start' };
    case 'P':
      return { kind: 'continuation' };
    case 'C':
      return { kind: 'command_start' };
    case 'D': {
      const exit = fieldValue(fields.slice(2), 'exit');
      if (exit === undefined || !/^-?\d+$/.test(exit)) {
        return null;
      }
      const ps = fieldValue(fields.slice(2), 'ps') ?? '';
      const pipestatus = ps
        .split(',')
        .filter((x) => /^-?\d+$/.test(x))
        .map((x) => Number(x));
      return { kind: 'command_end', exit: Number(exit), pipestatus };
    }
    default:
      return null;
  }
}

function fieldValue(fields: string[], key: string): string | undefined {
  const prefix = `${key}=`;
  const found = fields.find((f) => f.startsWith(prefix));
  return found === undefined ? undefined : found.slice(prefix.length);
}
