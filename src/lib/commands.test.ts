import { describe, expect, it } from 'vitest';

import {
  BUILTIN_COMMANDS,
  COMMAND_ALIASES,
  INIT_TEMPLATE,
  PLAN_TEMPLATE,
  REVIEW_TEMPLATE,
  conversationMarkdown,
  expandTemplate,
  findBuiltin,
  matchModel,
  parseCommand,
  renderContext,
  renderHelp,
  splitArguments,
  splitModelVariant,
} from './commands';
import type { ModelChoice } from './commands';
import type { CommandCatalog, ContextUsage } from './types';
import type { TranscriptItem } from './transcript';

describe('parseCommand', () => {
  it('parses a bare command with no args', () => {
    expect(parseCommand('/help')).toEqual({ name: 'help', args: '' });
  });

  it('parses a command with args', () => {
    expect(parseCommand('/plan add retry logic')).toEqual({ name: 'plan', args: 'add retry logic' });
  });

  it('lowercases the name and trims the args', () => {
    expect(parseCommand('/Model  openai/gpt-4o  ')).toEqual({ name: 'model', args: 'openai/gpt-4o' });
  });

  it('allows digits, underscores and dashes in names', () => {
    expect(parseCommand('/ok-name_1 x')).toEqual({ name: 'ok-name_1', args: 'x' });
    expect(parseCommand('/9lives x')).toEqual({ name: '9lives', args: 'x' });
  });

  it('returns null for plain text', () => {
    expect(parseCommand('hello')).toBeNull();
    expect(parseCommand('')).toBeNull();
    expect(parseCommand('use /help please')).toBeNull();
  });

  it('returns null for a lone slash and bad names', () => {
    expect(parseCommand('/')).toBeNull();
    expect(parseCommand('/-x')).toBeNull();
    expect(parseCommand('/_x')).toBeNull();
    expect(parseCommand('/a.b')).toBeNull();
  });

  it('is not a command when the name is followed by a path slash', () => {
    // The whole text is sent to the agent instead.
    expect(parseCommand('/usr/local')).toBeNull();
  });
});

describe('built-ins and aliases', () => {
  it('lists help, new, stop, model, export, context, history, rename, btw, compact, fork, plan, review and init', () => {
    expect(BUILTIN_COMMANDS.map((b) => b.name)).toEqual([
      'help',
      'new',
      'stop',
      'model',
      'export',
      'context',
      'history',
      'rename',
      'btw',
      'compact',
      'fork',
      'plan',
      'review',
      'init',
    ]);
  });

  it('finds a built-in and resolves the clear alias to new', () => {
    expect(findBuiltin('help')?.name).toBe('help');
    expect(findBuiltin('clear')?.name).toBe('new');
    expect(COMMAND_ALIASES.clear).toBe('new');
  });

  it('returns null for custom or unknown names', () => {
    expect(findBuiltin('deploy')).toBeNull();
    expect(findBuiltin('')).toBeNull();
  });
});

describe('splitArguments', () => {
  it('splits on whitespace', () => {
    expect(splitArguments('a b  c')).toEqual(['a', 'b', 'c']);
  });

  it('keeps quoted spans together', () => {
    expect(splitArguments('a "b c" \'d e\'')).toEqual(['a', 'b c', 'd e']);
  });

  it('handles backslash escapes', () => {
    expect(splitArguments('a\\ b c\\\\d "e\\"f"')).toEqual(['a b', 'c\\d', 'e"f']);
  });

  it('returns no tokens for blank args', () => {
    expect(splitArguments('')).toEqual([]);
    expect(splitArguments('   ')).toEqual([]);
  });
});

describe('expandTemplate', () => {
  it('replaces $ARGUMENTS with the whole args', () => {
    expect(expandTemplate('Do: $ARGUMENTS!', 'x y')).toBe('Do: x y!');
  });

  it('replaces $1..$9 with positional tokens', () => {
    expect(expandTemplate('$1 and $2', '"a b" c')).toBe('a b and c');
  });

  it('replaces a missing token with empty', () => {
    expect(expandTemplate('[$1][$3]', 'only')).toBe('[only][]');
  });

  it('appends the args when there is no placeholder', () => {
    expect(expandTemplate('Look at this.', 'the diff')).toBe('Look at this.\n\nARGUMENTS: the diff');
  });

  it('leaves the template alone with no placeholder and no args', () => {
    expect(expandTemplate('Look at this.', '')).toBe('Look at this.');
  });

  it('leaves other $ text alone', () => {
    expect(expandTemplate('Costs $0 and $10, not $x or $$.', 'a')).toBe('Costs $0 and $10, not $x or $$.\n\nARGUMENTS: a');
    expect(expandTemplate('$0 stays', '')).toBe('$0 stays');
  });

  it('the plan, review and init templates take the args', () => {
    expect(expandTemplate(PLAN_TEMPLATE, 'add retries')).toContain('add retries');
    expect(expandTemplate(PLAN_TEMPLATE, '')).not.toContain('$ARGUMENTS');
    expect(expandTemplate(REVIEW_TEMPLATE, 'auth')).toContain('auth');
    expect(expandTemplate(REVIEW_TEMPLATE, '')).not.toContain('$ARGUMENTS');
    expect(expandTemplate(INIT_TEMPLATE, 'tui')).toContain('tui');
    expect(expandTemplate(INIT_TEMPLATE, '')).not.toContain('$ARGUMENTS');
  });
});

const CHOICES: ModelChoice[] = [
  {
    ref: { provider_id: 'openai', model_id: 'gpt-4o' },
    label: 'OpenAI · gpt-4o',
    provider: 'OpenAI',
    variants: [
      { id: 'high', label: 'High' },
      { id: 'low', label: 'Low' },
    ],
  },
  {
    ref: { provider_id: 'other', model_id: 'gpt-4o' },
    label: 'Other · gpt-4o',
    provider: 'Other',
    variants: [{ id: 'high', label: 'High' }],
  },
  {
    ref: { provider_id: 'openai', model_id: 'o1' },
    label: 'OpenAI · o1',
    provider: 'OpenAI',
    variants: [{ id: 'high', label: 'High' }],
  },
];

describe('matchModel', () => {
  it('matches an exact provider_id/model_id', () => {
    const found = matchModel(CHOICES, 'openai/gpt-4o');
    expect(found).toEqual({ status: 'match', choice: CHOICES[0] });
  });

  it('matches an exact provider name/model_id', () => {
    const found = matchModel(CHOICES, 'Other/gpt-4o');
    expect(found).toEqual({ status: 'match', choice: CHOICES[1] });
  });

  it('matches a unique model_id', () => {
    const found = matchModel(CHOICES, 'o1');
    expect(found).toEqual({ status: 'match', choice: CHOICES[2] });
  });

  it('is ambiguous when the model_id is on several providers', () => {
    const found = matchModel(CHOICES, 'gpt-4o');
    expect(found.status).toBe('ambiguous');
    if (found.status === 'ambiguous') {
      expect(found.candidates).toEqual([CHOICES[0], CHOICES[1]]);
    }
  });

  it('matches nothing unknown, with no variants to offer', () => {
    expect(matchModel(CHOICES, 'nope')).toEqual({ status: 'none', variants: [] });
    expect(matchModel(CHOICES, 'openai/nope')).toEqual({ status: 'none', variants: [] });
  });

  it('no # returns the base ref', () => {
    const found = matchModel(CHOICES, 'openai/gpt-4o');
    expect(found).toEqual({ status: 'match', choice: CHOICES[0] });
    expect(found.status === 'match' ? found.choice.ref.variant : 'unset').toBeUndefined();
  });

  it('a #variant on a unique base returns the ref with the variant', () => {
    const found = matchModel(CHOICES, 'o1#high');
    expect(found).toEqual({
      status: 'match',
      choice: { ...CHOICES[2], ref: { provider_id: 'openai', model_id: 'o1', variant: 'high' } },
    });
  });

  it('a #variant on a provider/model returns the ref with the variant', () => {
    const found = matchModel(CHOICES, 'openai/gpt-4o#low');
    expect(found.status).toBe('match');
    if (found.status === 'match') {
      expect(found.choice.ref).toEqual({ provider_id: 'openai', model_id: 'gpt-4o', variant: 'low' });
    }
  });

  it('splits on the last #: a trailing # alone is the base', () => {
    expect(matchModel(CHOICES, 'o1#')).toEqual({ status: 'match', choice: CHOICES[2] });
  });

  it('an unknown variant is none, with the model’s provider/model#variant labels', () => {
    expect(matchModel(CHOICES, 'o1#faster')).toEqual({ status: 'none', variants: ['openai/o1#high'] });
    expect(matchModel(CHOICES, 'other/gpt-4o#low')).toEqual({ status: 'none', variants: ['other/gpt-4o#high'] });
  });

  it('a #variant shared by one of two same-named models picks that one', () => {
    const found = matchModel(CHOICES, 'gpt-4o#low');
    expect(found.status).toBe('match');
    if (found.status === 'match') {
      expect(found.choice.ref).toEqual({ provider_id: 'openai', model_id: 'gpt-4o', variant: 'low' });
    }
  });

  it('a #variant on several models stays ambiguous, with the variant on each candidate', () => {
    const found = matchModel(CHOICES, 'gpt-4o#high');
    expect(found.status).toBe('ambiguous');
    if (found.status === 'ambiguous') {
      expect(found.candidates.map((c) => c.ref)).toEqual([
        { provider_id: 'openai', model_id: 'gpt-4o', variant: 'high' },
        { provider_id: 'other', model_id: 'gpt-4o', variant: 'high' },
      ]);
    }
  });

  it('a #variant on no same-named model is none', () => {
    expect(matchModel(CHOICES, 'gpt-4o#faster')).toEqual({ status: 'none', variants: [] });
  });
});

describe('splitModelVariant', () => {
  it('splits on the last #', () => {
    expect(splitModelVariant('openai/gpt-4o#high')).toEqual({ base: 'openai/gpt-4o', variant: 'high' });
    expect(splitModelVariant('a#b#c')).toEqual({ base: 'a#b', variant: 'c' });
  });

  it('no # or a trailing # is the base', () => {
    expect(splitModelVariant('o1')).toEqual({ base: 'o1', variant: null });
    expect(splitModelVariant('o1#')).toEqual({ base: 'o1', variant: null });
  });
});

function catalogWith(commands: CommandCatalog['commands'], dirs: Pick<CommandCatalog, 'project_dir' | 'global_dir'>): CommandCatalog {
  return { commands, ...dirs };
}

describe('renderHelp', () => {
  it('lists the built-ins, the clear alias, the dirs and the unknown-text note', () => {
    const text = renderHelp(catalogWith([], { project_dir: '/w/.watt/commands', global_dir: '/cfg/commands' }));
    expect(text).toContain('/help — ');
    expect(text).toContain('/model [provider/model[#variant]] — ');
    expect(text).toContain('/context — ');
    expect(text).toContain('/history — ');
    expect(text).toContain('/rename <title> — ');
    expect(text).toContain('/btw <question> — ');
    expect(text).toContain('/compact — ');
    expect(text).toContain('/fork — ');
    expect(text).toContain('/plan [task] — ');
    expect(text).toContain('/review [focus] — ');
    expect(text).toContain('/init [focus] — ');
    expect(text).toContain('/clear — Alias for /new.');
    expect(text).toContain('- project: /w/.watt/commands');
    expect(text).toContain('- global: /cfg/commands');
    expect(text).toContain('Any other /text is sent to the agent unchanged.');
  });

  it('lists custom commands with their hint and source', () => {
    const text = renderHelp(
      catalogWith(
        [
          { name: 'deploy', description: 'Ship it.', hint: '[env]', source: 'project', template: 'Ship $1.' },
          { name: 'note', description: 'Take a note.', hint: null, source: 'global', template: 'Note: $ARGUMENTS' },
        ],
        { project_dir: '/w/.watt/commands', global_dir: '/cfg/commands' },
      ),
    );
    expect(text).toContain('/deploy [env] — Ship it. (project)');
    expect(text).toContain('/note — Take a note. (global)');
  });

  it('names dirs that do not apply', () => {
    const text = renderHelp(catalogWith([], { project_dir: null, global_dir: null }));
    expect(text).toContain('- project: (not available here)');
    expect(text).toContain('- global: (not available here)');
  });
});

describe('renderContext', () => {
  it('says no model call has happened yet', () => {
    expect(renderContext(null)).toBe('No model call in this conversation yet.');
  });

  it('reports usage against a known limit with a percent', () => {
    const text = renderContext({
      limit: 128000,
      used: 12800,
      reserved_output: 4000,
      trimmed: false,
      calibrated: false,
      categories: [],
    });
    expect(text).toContain('Context: 12,800 / 128,000 tokens (10%)');
    expect(text).toContain('Basis: estimate');
    expect(text).toContain('Reserved for the reply: 4,000 tokens');
    expect(text).toContain('Older messages left out of the last request: no');
    expect(text).toContain('Stored history is complete; trimming only changes what is sent.');
  });

  it('reports usage with an unknown limit', () => {
    const text = renderContext({
      limit: null,
      used: 5120,
      reserved_output: 1024,
      trimmed: true,
      calibrated: true,
      categories: [],
    });
    expect(text).toContain('Context: 5,120 tokens (model limit unknown)');
    expect(text).toContain('Basis: provider count');
    expect(text).toContain('Older messages left out of the last request: yes');
  });

  it('lists one indented row per category', () => {
    const usage: ContextUsage = {
      limit: 8000,
      used: 2000,
      reserved_output: 1000,
      trimmed: false,
      calibrated: false,
      categories: [
        { id: 'history', tokens: 1500 },
        { id: 'tools', tokens: 500 },
      ],
    };
    const text = renderContext(usage);
    expect(text).toContain('  history: 1,500 tokens');
    expect(text).toContain('  tools: 500 tokens');
  });
});

describe('conversationMarkdown', () => {
  it('exports user and agent blocks in order, skipping the rest', () => {
    const items: TranscriptItem[] = [
      { id: 1, kind: 'notice', text: 'hi' },
      { id: 2, kind: 'user', clientId: 1, text: 'do it' },
      { id: 3, kind: 'tool', callId: 'c1', name: 'run_command', args: null, status: 'ok', result: null, decision: 'read', reason: null },
      { id: 4, kind: 'agent', text: 'done', streaming: false },
      {
        id: 5,
        kind: 'approval',
        callId: 'c2',
        name: 'run_command',
        text: 'ls',
        display: null,
        state: 'pending',
        approve: null,
        reason: null,
        location: null,
      },
      {
        id: 6,
        kind: 'question',
        callId: 'c3',
        questions: [],
        state: 'pending',
        answers: null,
        result: null,
        error: null,
        note: null,
      },
      { id: 7, kind: 'record_end', recordId: 9, exit: 0 },
    ];
    expect(conversationMarkdown(items)).toBe('## You\n\ndo it\n\n## Agent\n\ndone');
  });

  it('lists attachment names under the user block', () => {
    const items: TranscriptItem[] = [
      {
        id: 1,
        kind: 'user',
        clientId: 1,
        text: 'look',
        attachments: [
          { kind: 'image', name: 'shot.png', bytes: 10 },
          { kind: 'file', name: 'notes.txt', bytes: 20 },
        ],
      },
    ];
    expect(conversationMarkdown(items)).toBe('## You\n\nlook\n\nAttachments: shot.png, notes.txt');
  });

  it('is empty for a conversation with no messages', () => {
    expect(conversationMarkdown([])).toBe('');
    expect(conversationMarkdown([{ id: 1, kind: 'notice', text: 'hi' }])).toBe('');
  });
});
