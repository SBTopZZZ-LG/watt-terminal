import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { SkillsSection, SKILL_DESCRIPTION_CHARS } from './SkillsSection';
import type { SkillsSectionProps } from './SkillsSection';
import type { SkillsView } from '../lib/types';

const NOOP = () => undefined;

const VIEW: SkillsView = {
  roots: ['C:/Users/me/.claude/skills', 'C:/Users/me/AppData/Roaming/Watt/skills'],
  warnings: ['broken/SKILL.md: missing description'],
  skills: [
    { name: 'review-pr', description: 'Review a pull request', source: 'user', path: 'C:/skills/review-pr', model_invocable: true },
    { name: 'deploy', description: 'Ship it', source: 'project', path: 'C:/repo/.claude/skills/deploy', model_invocable: false },
    { name: 'watt-help', description: 'Built in help', source: 'watt', path: 'builtin/watt-help', model_invocable: true },
  ],
};

const render = (over: Partial<SkillsSectionProps> = {}) =>
  renderToStaticMarkup(<SkillsSection view={VIEW} loadError={null} loading={false} onRefresh={NOOP} {...over} />);

describe('SkillsSection', () => {
  it('explains skills in two lines and names the folders it looks in', () => {
    const html = render();
    expect(html).toContain('A skill is a folder with a SKILL.md file');
    expect(html).toContain('Looked in');
    expect(html).toContain('C:/Users/me/.claude/skills');
    expect(html).toContain('C:/Users/me/AppData/Roaming/Watt/skills');
  });

  it('lists each skill with its source, and tags the ones hidden from the model', () => {
    const html = render();
    expect(html).toContain('review-pr');
    expect(html).toContain('your folder');
    expect(html).toContain('this project');
    expect(html).toContain('built in');
    expect(html.split('hidden from the model').length).toBe(2);
  });

  it('shows the skipped files as warnings', () => {
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
    const html = render({ view: { roots: [], warnings: [], skills: [] } });
    expect(html).toContain('No skills found in these folders yet.');
    expect(html).toContain('>Refresh<');
  });

  it('shows the loading line before the first read, and the read error', () => {
    expect(render({ view: null })).toContain('Loading skills...');
    const failed = render({ view: null, loadError: 'cannot read folders' });
    expect(failed).toContain('cannot read folders');
    expect(failed).not.toContain('Loading skills...');
  });

  it('Refresh is off while a read runs', () => {
    expect(render({ loading: true })).toContain('Refreshing...');
  });
});
