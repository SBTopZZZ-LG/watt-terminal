import { useEffect, useId, useRef, useState } from 'react';
import { api, errorText } from '../lib/api';
import { capDisplay, displayText } from '../lib/keysDisplay';
import type { SkillView, SkillsView } from '../lib/types';

/** The most characters of a skill's description the list shows. The whole text is in its SKILL.md. */
export const SKILL_DESCRIPTION_CHARS = 200;

const SOURCE_LABEL: Record<SkillView['source'], string> = {
  watt: 'built in',
  user: 'your folder',
  project: 'this project',
};

export interface SkillsSectionProps {
  /** The skills as last read, or null before the first read. */
  view: SkillsView | null;
  loadError: string | null;
  loading: boolean;
  onRefresh(): void;
}

/**
 * The Skills tab: how skills work, the folders Watt looks in, the skills found, and the files it skipped. Read-only: there are
 * no toggles here. A skill marked hidden is in the list but not offered to the model.
 */
export function SkillsSection({ view, loadError, loading, onRefresh }: SkillsSectionProps) {
  const uid = useId();
  return (
    <section className="settings-section" aria-labelledby={`${uid}-title`}>
      <h3 className="settings-section-title" id={`${uid}-title`}>
        Skills
      </h3>
      <p className="settings-hint">
        A skill is a folder with a SKILL.md file: its name, a description and instructions. The model sees the list of skills and loads one with the
        skill tool when it fits the task.
      </p>
      <p className="settings-hint">Put a folder with a SKILL.md in one of the folders below, then press Refresh.</p>

      {view !== null && view.roots.length > 0 && (
        <div className="settings-mcp-roots">
          <span className="settings-label">Looked in</span>
          <ul>
            {view.roots.map((root) => (
              <li key={root}>
                <code>{displayText(root)}</code>
              </li>
            ))}
          </ul>
        </div>
      )}

      <div className="settings-inline">
        <button type="button" disabled={loading} onClick={onRefresh}>
          {loading ? 'Refreshing...' : 'Refresh'}
        </button>
      </div>

      {loadError !== null && (
        <p className="settings-error" role="alert">
          {loadError}
        </p>
      )}
      {view === null && loadError === null && <p className="settings-hint">Loading skills...</p>}
      {view !== null && view.skills.length === 0 && (
        <p className="settings-hint">No skills found in these folders yet.</p>
      )}

      {view !== null && view.skills.length > 0 && (
        <ul className="settings-skills">
          {view.skills.map((skill) => (
            <li key={skill.path} className="settings-skill">
              <div className="settings-inline">
                <span className="settings-row-name">{displayText(skill.name)}</span>
                <span className="settings-tag settings-tag-muted">{SOURCE_LABEL[skill.source]}</span>
                {!skill.model_invocable && <span className="settings-tag settings-tag-warn">hidden from the model</span>}
              </div>
              <span className="settings-dim">{capDisplay(skill.description, SKILL_DESCRIPTION_CHARS)}</span>
              <span className="settings-dim">
                <code>{displayText(skill.path)}</code>
              </span>
            </li>
          ))}
        </ul>
      )}

      {view !== null && view.warnings.length > 0 && (
        <div className="settings-warn-list" role="status">
          <span className="settings-label">Skipped</span>
          <ul>
            {view.warnings.map((w) => (
              <li key={w} className="settings-warn">
                {displayText(w)}
              </li>
            ))}
          </ul>
        </div>
      )}
    </section>
  );
}

/**
 * The Skills tab's container: reads the skills when the tab is first shown, and again on Refresh. There is no polling: the
 * folders change only when the user changes them, and Refresh is the way to see that.
 */
export function SkillsTab({ active }: { active: boolean }) {
  const [view, setView] = useState<SkillsView | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const seq = useRef(0);
  const applied = useRef(0);

  const read = () => {
    const my = ++seq.current;
    setLoading(true);
    api.listSkills(null).then(
      (v) => {
        if (my > applied.current) {
          applied.current = my;
          setView(v);
          setLoadError(null);
        }
        setLoading(false);
      },
      (e: unknown) => {
        if (my > applied.current) {
          setLoadError(errorText(e));
        }
        setLoading(false);
      },
    );
  };

  useEffect(() => {
    if (active && view === null && loadError === null) {
      read();
    }
    // Read once, the first time the tab is shown. After that the tab reads again only on Refresh.
  }, [active]);

  return <SkillsSection view={view} loadError={loadError} loading={loading} onRefresh={read} />;
}
