import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { FolderSheet, SkillsSection, SKILL_DESCRIPTION_CHARS } from './SkillsSection';
import type { SkillsSectionProps } from './SkillsSection';
import type { SkillView, SkillsView } from '../lib/types';

const NOOP = () => undefined;

const VIEW: SkillsView = {
  roots: [
    { path: 'C:/Users/me/AppData/Roaming/Watt/skills', exists: true, source: 'watt' },
    { path: 'C:/Users/me/.claude/skills', exists: false, source: 'user' },
  ],
  folders: [
    { folder: '~/.claude/skills', path: 'C:/Users/me/.claude/skills', status: 'not_found' },
    { folder: 'C:/Users/me/tools', path: 'C:/Users/me/tools', status: 'blocked' },
  ],
  using_defaults: false,
  disabled: ['deploy'],
  approval: 'auto',
  overrides: {},
  warnings: ['broken/SKILL.md: missing description'],
  skills: [
    { name: 'review-pr', description: 'Review a pull request', source: 'user', path: 'C:/skills/review-pr', model_invocable: true, enabled: true, approval: 'auto', override: null, shadowed_by: null },
    { name: 'deploy', description: 'Ship it', source: 'project', path: 'C:/repo/.claude/skills/deploy', model_invocable: false, enabled: false, approval: 'auto', override: null, shadowed_by: null },
    { name: 'watt-help', description: 'Built in help', source: 'watt', path: 'C:/cfg/skills/watt-help', model_invocable: true, enabled: true, approval: 'auto', override: null, shadowed_by: null },
    { name: 'review-pr', description: 'Old copy', source: 'project', path: 'C:/repo/.agents/skills/review-pr', model_invocable: true, enabled: true, approval: 'auto', override: null, shadowed_by: 'C:/skills' },
  ],
};

const render = (over: Partial<SkillsSectionProps> = {}) =>
  renderToStaticMarkup(
    <SkillsSection
      view={VIEW}
      loadError={null}
      loading={false}
      saving={false}
      saveError={null}
      onRefresh={NOOP}
      onSave={NOOP}
      {...over}
    />,
  );

describe('SkillsSection', () => {
  it('explains skills and where the folders are read from', () => {
    const html = render();
    expect(html).toContain('A skill is a folder with a SKILL.md file');
    expect(html).toContain('Folders starting with ~ are in your home folder');
  });

  it('shows the Watt folder locked first, then the configured folders with their status', () => {
    const html = render();
    expect(html).toContain('watt config');
    expect(html).toContain('C:/Users/me/AppData/Roaming/Watt/skills');
    expect(html).toContain('>found</span><span class="settings-tag settings-tag-muted">locked</span>');
    expect(html).toContain('>~/.claude/skills</code>');
    expect(html).toContain('>C:/Users/me/tools</code>');
    expect(html).toContain('not found');
    expect(html).toContain('blocked');
    expect(html).toContain('aria-label="Edit C:/Users/me/tools"');
  });

  it('shows the resolved path under a folder only when it differs from the text as configured', () => {
    const html = render();
    // "~/.claude/skills" resolves to a different path: shown. "C:/Users/me/tools" is its own path: not repeated.
    expect(html.split('class="settings-path"').length).toBe(2);
    expect(html).toContain('class="settings-path">C:/Users/me/.claude/skills</span>');
  });

  it('a long folder or path is cut on one line with its full text in the title', () => {
    const long = `C:/Users/someone/${'deeply-nested-folder/'.repeat(12)}skills`;
    const html = render({ view: { ...VIEW, folders: [{ folder: long, path: long, status: 'found' }] } });
    expect(html).toContain(`class="settings-truncate" title="${long}">${long}</code>`);
    expect(html).toContain('settings-truncate');
  });

  it('lists each skill with its source, a switch, and the tag for a hidden one', () => {
    const html = render();
    expect(html).toContain('review-pr');
    expect(html).toContain('your folder');
    expect(html).toContain('this project');
    expect(html).not.toContain('built in');
    expect(html.split('hidden from the model').length).toBe(2);
    // One switch per skill in use (three); none for the shadowed one.
    expect(html.split('type="checkbox"').length).toBe(4);
    expect(html).toContain('switched off');
  });

  it('shows the global approval level and a level per skill in use, not for the shadowed one', () => {
    const html = render({ view: { ...VIEW, approval: 'ask', overrides: { 'watt-help': 'auto' }, skills: VIEW.skills.map((s): SkillView => (s.name === 'watt-help' ? { ...s, approval: 'auto', override: 'auto' } : { ...s, approval: 'ask' })) } });
    expect(html).toContain('Loading a skill');
    expect(html).toContain('<option value="ask" selected="">Ask first</option>');
    expect(html).toContain('<option value="auto">Don&#x27;t ask</option>');
    // The global select, then one select per skill in use (three).
    expect(html.split('<select').length).toBe(5);
    expect(html).toContain('aria-label="Loading review-pr"');
    expect(html).toContain('<option value="global">Global default (Ask first)</option>');
    expect(html).toContain('<option value="auto" selected="">Don&#x27;t ask</option>');
  });

  it('marks a shadowed skill with its note and no switch', () => {
    const html = render();
    expect(html).toContain('shadowed');
    expect(html).toContain('Hidden: the one in C:/skills is used instead.');
    expect(html).toContain('settings-skill-muted');
  });

  it('shows the skipped and refused warnings', () => {
    expect(render()).toContain('broken/SKILL.md: missing description');
  });

  it('a description longer than the cap is cut and marked', () => {
    const long = 'word '.repeat(100);
    const html = render({ view: { ...VIEW, skills: [{ ...VIEW.skills[0], description: long }] } });
    expect(html).toContain('…');
    // The text shown is cut; the full text is only in the title, for the hover.
    expect(html).not.toContain(`>${long.trim()}<`);
    expect(html).toContain(`title="${long.trim()}`);
    expect(html).toContain('settings-clamp');
    expect(SKILL_DESCRIPTION_CHARS).toBe(200);
  });

  it('a shadowed skill is muted and has its note', () => {
    const html = render();
    expect(html).toContain('settings-card-muted');
    expect(html).toContain('Hidden: the one in C:/skills is used instead.');
  });

  it('shows the approval hint in plain words', () => {
    const html = render();
    expect(html).toContain('Ask first: a card asks before a skill is loaded the first time in a turn.');
  });

  it('shows the folders searched when no skill is found', () => {
    const html = render({ view: { ...VIEW, roots: [{ path: 'C:/cfg/skills', exists: false, source: 'watt' }], skills: [], warnings: [] } });
    expect(html).toContain('Searched:');
    expect(html).toContain('>C:/cfg/skills</code>');
  });

  it('a switched-off skill is dimmed and its approval select is off', () => {
    const html = render();
    expect(html).toContain('settings-card settings-skill is-off');
    expect(html).toMatch(/<select aria-label="Loading deploy" disabled="">/);
  });

  it('says when no skills were found, and offers Refresh', () => {
    const html = render({ view: { ...VIEW, roots: [], folders: [], skills: [], warnings: [], disabled: [] } });
    expect(html).toContain('No skills found in these folders yet.');
    expect(html).toContain('>Refresh<');
  });

  it('offers Reset to defaults and disables it while the defaults apply and nothing changed', () => {
    const defaults = render({ view: { ...VIEW, using_defaults: true } });
    expect(defaults).toMatch(/disabled=""[^>]*>Reset to defaults/);
    expect(render()).not.toMatch(/disabled=""[^>]*>Reset to defaults/);
  });

  it('shows the loading line before the first read, and the read error', () => {
    expect(render({ view: null })).toContain('Loading skills...');
    const failed = render({ view: null, loadError: 'cannot read folders' });
    expect(failed).toContain('cannot read folders');
    expect(failed).not.toContain('Loading skills...');
  });

  it('shows a save error and turns the switches off while a save runs', () => {
    expect(render({ saveError: 'skill folder 1 is empty' })).toContain('skill folder 1 is empty');
    const saving = render({ saving: true });
    expect(saving.split('disabled=""').length).toBeGreaterThan(4);
  });

  it('Refresh is off while a read runs', () => {
    expect(render({ loading: true })).toContain('Refreshing...');
  });
});

describe('FolderSheet', () => {
  const sheet = (index: number | null, draft: string, extra: Partial<{ submitted: boolean; saving: boolean; saveError: string | null }> = {}) =>
    renderToStaticMarkup(
      <FolderSheet
        view={VIEW}
        sheet={{ index, draft, submitted: extra.submitted ?? false }}
        saving={extra.saving ?? false}
        saveError={extra.saveError ?? null}
        onDraft={NOOP}
        onSubmit={NOOP}
        onClose={NOOP}
      />,
    );

  it('an add sheet is titled for adding, focuses its input, and cannot save an empty folder', () => {
    const html = sheet(null, '');
    expect(html).toContain('Add skill folder');
    expect(html).toContain('data-autofocus');
    expect(html).toContain('aria-label="Skill folder"');
    expect(html).toMatch(/disabled=""[^>]*>Add folder<\/button>/);
    expect(html).toContain('A folder that is not there yet is kept.');
  });

  it('offers Browse beside the path in both sheets, and holds it off while a save runs', () => {
    expect(sheet(null, '')).toMatch(/<button type="button"[^>]*>Browse…<\/button>/);
    expect(sheet(0, '~/.claude/skills')).toMatch(/<button type="button"[^>]*>Browse…<\/button>/);
    expect(sheet(0, '~/.claude/skills', { saving: true })).toMatch(/disabled=""[^>]*>Browse…<\/button>/);
  });

  it('an edit sheet is off Save until the folder changes', () => {
    expect(sheet(0, '~/.claude/skills')).toMatch(/disabled=""[^>]*>Save folder<\/button>/);
    expect(sheet(0, '~/other')).not.toMatch(/disabled=""[^>]*>Save folder<\/button>/);
    expect(sheet(0, '~/other')).toContain('Skill folder</h3>');
  });

  it('shows why a folder cannot be kept, and a save error only after a save was sent', () => {
    expect(sheet(null, '~/a')).not.toContain('role="alert"');
    expect(sheet(null, 'a\0b')).toContain('A folder has a NUL character.');
    expect(sheet(null, '~/.claude/skills')).toContain('That folder is already in the list.');
    expect(sheet(1, '~/.claude/skills')).toContain('aria-invalid');
    expect(sheet(null, '~/x', { submitted: true, saveError: 'boom' })).toContain('boom');
    expect(sheet(null, '~/x', { saveError: 'boom' })).not.toContain('boom');
  });
});
