import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { SkillsSection, SKILL_DESCRIPTION_CHARS } from './SkillsSection';
import type { SkillsSectionProps } from './SkillsSection';
import type { SkillsView } from '../lib/types';

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
  warnings: ['broken/SKILL.md: missing description'],
  skills: [
    { name: 'review-pr', description: 'Review a pull request', source: 'user', path: 'C:/skills/review-pr', model_invocable: true, enabled: true, shadowed_by: null },
    { name: 'deploy', description: 'Ship it', source: 'project', path: 'C:/repo/.claude/skills/deploy', model_invocable: false, enabled: false, shadowed_by: null },
    { name: 'watt-help', description: 'Built in help', source: 'watt', path: 'C:/cfg/skills/watt-help', model_invocable: true, enabled: true, shadowed_by: null },
    { name: 'review-pr', description: 'Old copy', source: 'project', path: 'C:/repo/.agents/skills/review-pr', model_invocable: true, enabled: true, shadowed_by: 'C:/skills' },
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
    expect(html).toContain('found, locked');
    expect(html).toContain('value="~/.claude/skills"');
    expect(html).toContain('value="C:/Users/me/tools"');
    expect(html).toContain('not found');
    expect(html).toContain('blocked');
    expect(html).toContain('aria-label="Skill folder 2"');
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
    expect(html).not.toContain(long.trim());
    expect(SKILL_DESCRIPTION_CHARS).toBe(200);
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
