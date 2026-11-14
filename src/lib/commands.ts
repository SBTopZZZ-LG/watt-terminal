// Slash commands typed in the agent bar: local actions (`/help`, `/new`, `/stop`, `/model`, `/export`,
// `/context`, `/history`, `/rename`, `/btw`, `/compact`, `/fork`) and prompt templates (`/plan`, `/review`, `/init`,
// custom commands) expanded into a normal user message. Pure logic; App.tsx runs them.
// Unknown `/...` text is not a command: the full text is sent to the agent unchanged. Built-ins win over customs.

import type { CommandCatalog, ContextUsage, ModelRef } from './types';

/** A `/name args` line. `args` is the rest of the line, trimmed (empty when there is none). */
export interface ParsedCommand {
  name: string;
  args: string;
}

/**
 * Parses a bar line as a command: it starts with `/`, then a name (a letter or digit, then letters, digits, `_`
 * or `-`), then whitespace or the end. The name is lowercased. Anything else (plain text, `/usr/local`, a lone
 * `/`) is null: the full text goes to the agent.
 */
export function parseCommand(text: string): ParsedCommand | null {
  const line = text.trimStart();
  if (!line.startsWith('/')) {
    return null;
  }
  const match = /^\/([A-Za-z0-9][A-Za-z0-9_-]*)(?:\s|$)/.exec(line);
  if (!match) {
    return null;
  }
  return { name: match[1].toLowerCase(), args: line.slice(match[0].length).trim() };
}

/** One built-in command, as `/help` shows it. `hint` is the argument placeholder, null when it takes none. */
export interface BuiltinCommand {
  name: string;
  description: string;
  hint: string | null;
}

export const BUILTIN_COMMANDS: BuiltinCommand[] = [
  { name: 'help', description: 'List the slash commands and where custom commands live.', hint: null },
  { name: 'new', description: 'Start a new conversation (the old one stays in history).', hint: null },
  { name: 'stop', description: 'Stop the running turn.', hint: null },
  { name: 'model', description: 'With no argument, open Settings; with one, switch the default model.', hint: '[provider/model[#variant]]' },
  { name: 'export', description: 'Copy the conversation to the clipboard as Markdown.', hint: null },
  { name: 'context', description: 'Show the context usage of the last model request.', hint: null },
  { name: 'history', description: 'Open the History view.', hint: null },
  { name: 'rename', description: 'Rename the current conversation.', hint: '<title>' },
  { name: 'btw', description: 'Ask a side question about the conversation; the answer is not stored.', hint: '<question>' },
  { name: 'compact', description: 'Summarize the conversation into a new one; the old one stays in history.', hint: null },
  { name: 'fork', description: 'Copy the conversation into a new one and switch to it.', hint: null },
  { name: 'plan', description: 'Ask for a plan first (guidance only; approvals still apply).', hint: '[task]' },
  { name: 'review', description: 'Ask for a review of the working tree (guidance only; approvals still apply).', hint: '[focus]' },
  { name: 'init', description: 'Ask the agent to create or update AGENTS.md at the workspace root.', hint: '[focus]' },
];

/** Aliases to built-ins, by lowercased name. */
export const COMMAND_ALIASES: Record<string, string> = {
  clear: 'new',
};

/** The built-in for a lowercased command name, following aliases. Null for custom or unknown names. */
export function findBuiltin(name: string): BuiltinCommand | null {
  const key = name.toLowerCase();
  const direct = BUILTIN_COMMANDS.find((b) => b.name === key);
  if (direct !== undefined) {
    return direct;
  }
  const aliased = COMMAND_ALIASES[key];
  if (aliased === undefined) {
    return null;
  }
  return BUILTIN_COMMANDS.find((b) => b.name === aliased) ?? null;
}

/** `/plan <task>`: investigate and reply with a concise plan; no edits or mutating commands until approved. */
export const PLAN_TEMPLATE = `Investigate the request below and reply with a concise plan. Do not make any edits and do not run any mutating commands until the plan is approved.

$ARGUMENTS

Structure the reply as: goal, files and commands to look at, risks, and acceptance checks.`;

/** `/review <focus>`: review the working tree; report correctness bugs first; do not modify anything. */
export const REVIEW_TEMPLATE = `Review the working tree for the focus below. Do not modify anything.

$ARGUMENTS

Use run_command for git status and the diff, read the changed files, and report correctness bugs first, then smaller issues.`;

/** `/init [focus]`: inspect the project, then create or update AGENTS.md at the workspace root. Writes via the shell. */
export const INIT_TEMPLATE = `Inspect this project and create or update AGENTS.md at the workspace root.

$ARGUMENTS

Read the README, manifests, tests, scripts and CI config to learn what the project is, its build/test/lint commands, directory map, conventions and invariants, and things not to break. Keep existing useful guidance. Write the file via the shell (approval applies) and change nothing else.`;

/**
 * Splits command arguments into positional tokens: whitespace-separated, with `"double"` and `'single'` quotes and
 * backslash escapes (`\"`, `\\`, `\ `). An empty quoted string is an empty token.
 */
export function splitArguments(args: string): string[] {
  const tokens: string[] = [];
  let current = '';
  let quote: '"' | "'" | null = null;
  let inToken = false;
  let i = 0;
  while (i < args.length) {
    const c = args[i];
    if (quote !== null) {
      if (c === '\\' && quote === '"' && i + 1 < args.length) {
        current += args[i + 1];
        i += 2;
        continue;
      }
      if (c === quote) {
        quote = null;
        i += 1;
        continue;
      }
      current += c;
      i += 1;
      continue;
    }
    if (c === '"' || c === "'") {
      quote = c;
      inToken = true;
      i += 1;
      continue;
    }
    if (c === '\\' && i + 1 < args.length) {
      current += args[i + 1];
      inToken = true;
      i += 2;
      continue;
    }
    if (/\s/.test(c)) {
      if (inToken) {
        tokens.push(current);
        current = '';
        inToken = false;
      }
      i += 1;
      continue;
    }
    current += c;
    inToken = true;
    i += 1;
  }
  if (inToken) {
    tokens.push(current);
  }
  return tokens;
}

/**
 * Expands a command template: `$ARGUMENTS` is the whole args, `$1`..`$9` are the positional tokens (a missing one
 * is empty). With no placeholder and non-empty args, the args are appended as `ARGUMENTS: <args>`. Any other `$`
 * text (`$0`, `$10`, `$x`, `$$`) is left alone.
 */
export function expandTemplate(template: string, args: string): string {
  const tokens = splitArguments(args);
  let replaced = false;
  const expanded = template.replace(/\$ARGUMENTS|\$[1-9](?![0-9])/g, (marker) => {
    replaced = true;
    if (marker === '$ARGUMENTS') {
      return args;
    }
    return tokens[Number(marker.slice(1)) - 1] ?? '';
  });
  if (!replaced && args !== '') {
    return `${template}\n\nARGUMENTS: ${args}`;
  }
  return expanded;
}

/** One model the `/model` argument can match: its ref, its `Provider · model` label, the provider's name, and its variants. */
export interface ModelChoice {
  ref: ModelRef;
  label: string;
  provider: string;
  variants: { id: string; label: string }[];
}

export type ModelMatch =
  | { status: 'match'; choice: ModelChoice }
  | { status: 'none'; variants: string[] }
  | { status: 'ambiguous'; candidates: ModelChoice[] };

/**
 * Splits a `/model` argument on the last `#`: the base plus the requested variant id, or null when there is none (a
 * trailing `#` alone is the base). `openai/gpt#high` is base `openai/gpt`, variant `high`.
 */
export function splitModelVariant(arg: string): { base: string; variant: string | null } {
  const hash = arg.lastIndexOf('#');
  if (hash < 0) {
    return { base: arg, variant: null };
  }
  const variant = arg.slice(hash + 1);
  return variant === '' ? { base: arg.slice(0, hash), variant: null } : { base: arg.slice(0, hash), variant };
}

/** The choice with its ref carrying the variant: `provider/model#variant` runs that variant, null the base model. */
function withVariant(choice: ModelChoice, variant: string | null): ModelChoice {
  return { ...choice, ref: { ...choice.ref, variant } };
}

/**
 * Matches a `/model` argument against the saved models: an exact `provider_id/model_id`, an exact
 * `provider_name/model_id`, or a unique exact `model_id`, each optionally followed by `#variant`. One hit matches;
 * several are ambiguous; none is none. A requested variant must exist on the matched model: otherwise the match is
 * none, with that model's `provider/model#variant` labels as `variants`.
 */
export function matchModel(options: ModelChoice[], arg: string): ModelMatch {
  const { base, variant } = splitModelVariant(arg.trim());
  const slash = base.indexOf('/');
  const matches =
    slash < 0
      ? options.filter((o) => o.ref.model_id === base)
      : options.filter((o) => `${o.ref.provider_id}/${o.ref.model_id}` === base || `${o.provider}/${o.ref.model_id}` === base);
  if (matches.length === 1) {
    const only = matches[0];
    if (variant === null) {
      return { status: 'match', choice: only };
    }
    if (only.variants.some((v) => v.id === variant)) {
      return { status: 'match', choice: withVariant(only, variant) };
    }
    return { status: 'none', variants: only.variants.map((v) => `${only.ref.provider_id}/${only.ref.model_id}#${v.id}`) };
  }
  if (matches.length === 0) {
    return { status: 'none', variants: [] };
  }
  if (variant === null) {
    return { status: 'ambiguous', candidates: matches };
  }
  const narrowed = matches.filter((o) => o.variants.some((v) => v.id === variant));
  if (narrowed.length === 1) {
    return { status: 'match', choice: withVariant(narrowed[0], variant) };
  }
  if (narrowed.length === 0) {
    return { status: 'none', variants: [] };
  }
  return { status: 'ambiguous', candidates: narrowed.map((o) => withVariant(o, variant)) };
}

/**
 * The `/help` text: the built-ins (with the `/clear` alias), then the custom commands with their source, then the
 * skipped command files with their reasons, then the two command directories (reported even when missing), and the
 * note that unknown `/text` goes to the agent.
 */
export function renderHelp(catalog: CommandCatalog): string {
  const lines = ['Slash commands:'];
  for (const builtin of BUILTIN_COMMANDS) {
    lines.push(`/${builtin.name}${builtin.hint ? ` ${builtin.hint}` : ''} — ${builtin.description}`);
  }
  lines.push('/clear — Alias for /new.');
  if (catalog.commands.length > 0) {
    lines.push('', 'Custom commands:');
    for (const cmd of catalog.commands) {
      lines.push(`/${cmd.name}${cmd.hint ? ` ${cmd.hint}` : ''} — ${cmd.description} (${cmd.source})`);
    }
  }
  if ((catalog.skipped ?? []).length > 0) {
    lines.push('', 'Skipped custom command files:');
    for (const entry of catalog.skipped ?? []) {
      lines.push(`- ${entry}`);
    }
  }
  lines.push(
    '',
    'Custom commands live in:',
    `- project: ${catalog.project_dir ?? '(not available here)'}`,
    `- global: ${catalog.global_dir ?? '(not available here)'}`,
    '',
    'Any other /text is sent to the agent unchanged.',
  );
  return lines.join('\n');
}

/**
 * The `/context` text: the context usage of the last model request, or that no model call has happened yet.
 * Tokens use `en-US` grouping; the percent is the share of the known limit. The stored history stays complete:
 * trimming only changes what is sent.
 */
export function renderContext(usage: ContextUsage | null): string {
  if (usage === null) {
    return 'No model call in this conversation yet.';
  }
  const tokens = (n: number): string => n.toLocaleString('en-US');
  const lines: string[] = [];
  if (usage.limit === null || usage.limit <= 0) {
    lines.push(`Context: ${tokens(usage.used)} tokens (model limit unknown)`);
  } else {
    lines.push(`Context: ${tokens(usage.used)} / ${tokens(usage.limit)} tokens (${Math.round((usage.used / usage.limit) * 100)}%)`);
  }
  lines.push(`Basis: ${usage.calibrated ? 'provider count' : 'estimate'}`);
  lines.push(`Reserved for the reply: ${tokens(usage.reserved_output)} tokens`);
  lines.push(`Older messages left out of the last request: ${usage.trimmed ? 'yes' : 'no'}`);
  for (const category of usage.categories) {
    lines.push(`  ${category.id}: ${tokens(category.tokens)} tokens`);
  }
  lines.push('Stored history is complete; trimming only changes what is sent.');
  return lines.join('\n');
}
