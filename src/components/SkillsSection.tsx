import { useEffect, useId, useRef, useState } from 'react';
import { api, errorText } from '../lib/api';
import { capDisplay, displayText } from '../lib/keysDisplay';
import {
  FOLDER_STATUS_LABEL,
  SOURCE_LABEL,
  addFolder,
  removeFolder,
  rowsProblem,
  saveInput,
  setFolder,
  setSwitch,
  shadowNote,
} from '../lib/skillFolders';
import type { SkillsSettingsInput, SkillsView } from '../lib/types';

/** The most characters of a skill's description the list shows. The whole text is in its SKILL.md. */
export const SKILL_DESCRIPTION_CHARS = 200;

export interface SkillsSectionProps {
  /** The skills as last read, or null before the first read. */
  view: SkillsView | null;
  loadError: string | null;
  loading: boolean;
  /** A save is running: every control that changes the settings is off. */
  saving: boolean;
  saveError: string | null;
  onRefresh(): void;
  /** Saves the folder settings and the switches. The container re-reads the view after it succeeds. */
  onSave(input: SkillsSettingsInput): void;
}

/** The folder settings as they are saved: the folders as configured, or null while the defaults apply. */
function savedFolders(view: SkillsView): string[] | null {
  return view.using_defaults ? null : view.folders.map((f) => f.folder);
}

/**
 * The Skills tab: how skills work, the folders Watt reads them from (the Watt folder is locked, the others are edited here),
 * the skills found with a switch each, and the warnings. A skill marked hidden is in the list but never offered to the model.
 */
export function SkillsSection({ view, loadError, loading, saving, saveError, onRefresh, onSave }: SkillsSectionProps) {
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
      <p className="settings-hint">
        Folders starting with ~ are in your home folder, absolute paths are used as they are, and other relative paths are read from the folder of the
        current terminal session. A folder that is not there yet is kept.
      </p>

      {view !== null && <FolderEditor view={view} saving={saving} onSave={onSave} />}

      <div className="settings-inline">
        <button type="button" disabled={loading || saving} onClick={onRefresh}>
          {loading ? 'Refreshing...' : 'Refresh'}
        </button>
      </div>

      {saveError !== null && (
        <p className="settings-error" role="alert">
          {saveError}
        </p>
      )}
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
          {view.skills.map((skill) => {
            const hidden = skill.shadowed_by !== null;
            return (
              <li key={skill.path} className={hidden ? 'settings-skill settings-skill-muted' : 'settings-skill'}>
                <div className="settings-inline">
                  {!hidden && (
                    <label className="settings-check">
                      <input
                        type="checkbox"
                        checked={skill.enabled}
                        disabled={saving}
                        onChange={(e) => onSave({ folders: savedFolders(view), disabled: setSwitch(view.disabled, skill.name, e.target.checked) })}
                      />
                      Use
                    </label>
                  )}
                  <span className="settings-row-name">{displayText(skill.name)}</span>
                  <span className="settings-tag settings-tag-muted">{SOURCE_LABEL[skill.source]}</span>
                  {!hidden && !skill.enabled && <span className="settings-tag settings-tag-muted">switched off</span>}
                  {!skill.model_invocable && <span className="settings-tag settings-tag-warn">hidden from the model</span>}
                  {hidden && <span className="settings-tag settings-tag-muted">shadowed</span>}
                </div>
                {hidden && <span className="settings-dim">{displayText(shadowNote(skill.shadowed_by ?? ''))}</span>}
                <span className="settings-dim">{capDisplay(skill.description, SKILL_DESCRIPTION_CHARS)}</span>
                <span className="settings-dim">
                  <code>{displayText(skill.path)}</code>
                </span>
              </li>
            );
          })}
        </ul>
      )}

      {view !== null && view.warnings.length > 0 && (
        <div className="settings-warn-list" role="status">
          <span className="settings-label">Warnings</span>
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
 * The folder list: the locked Watt folder, then the configured folders, each editable, with its status. Changes wait for Save
 * folders. Reset to defaults saves the defaults at once. The rows follow the saved folders, so a switch saved elsewhere keeps
 * an unsaved edit here.
 */
function FolderEditor({ view, saving, onSave }: { view: SkillsView; saving: boolean; onSave(input: SkillsSettingsInput): void }) {
  const saved = view.folders.map((f) => f.folder);
  const [rows, setRows] = useState<string[]>(saved);
  const [draft, setDraft] = useState('');
  const [error, setError] = useState<string | null>(null);
  const savedKey = JSON.stringify([saved, view.using_defaults]);
  useEffect(() => {
    setRows(saved);
    setError(null);
    // Follows the saved folders only (see savedKey).
  }, [savedKey]);

  const watt = view.roots.find((r) => r.source === 'watt');
  const dirty = rows.length !== saved.length || rows.some((row, i) => row !== saved[i]);
  const statusOf = (i: number) => {
    const entry = view.folders[i];
    return entry !== undefined && entry.folder === rows[i] ? entry : undefined;
  };

  const save = () => {
    const problem = rowsProblem(rows);
    if (problem !== null) {
      setError(problem);
      return;
    }
    onSave(saveInput(rows, view.disabled));
  };

  return (
    <div className="settings-skill-folders">
      <span className="settings-label">Skill folders</span>
      <ul>
        {watt !== undefined && (
          <li className="settings-skill-folder">
            <span className="settings-tag settings-tag-muted">{SOURCE_LABEL.watt}</span>
            <code>{displayText(watt.path)}</code>
            <span className="settings-dim">{watt.exists ? 'found' : 'not found'}, locked</span>
          </li>
        )}
        {rows.map((row, i) => {
          const entry = statusOf(i);
          return (
            <li key={i} className="settings-skill-folder">
              <input
                aria-label={`Skill folder ${i + 1}`}
                value={row}
                disabled={saving}
                onChange={(e) => setRows(setFolder(rows, i, e.target.value))}
              />
              <span className="settings-dim">{entry !== undefined ? FOLDER_STATUS_LABEL[entry.status] : 'not saved yet'}</span>
              {entry?.path ? <code>{displayText(entry.path)}</code> : null}
              <button type="button" disabled={saving} onClick={() => setRows(removeFolder(rows, i))}>
                Remove
              </button>
            </li>
          );
        })}
      </ul>
      <div className="settings-inline">
        <input
          aria-label="New skill folder"
          placeholder="~/my-skills or a path"
          value={draft}
          disabled={saving}
          onChange={(e) => setDraft(e.target.value)}
        />
        <button
          type="button"
          disabled={saving || draft.trim() === ''}
          onClick={() => {
            const next = addFolder(rows, draft);
            setRows(next.rows);
            setError(next.error);
            if (next.error === null) setDraft('');
          }}
        >
          Add folder
        </button>
      </div>
      {error !== null && (
        <p className="settings-error" role="alert">
          {error}
        </p>
      )}
      <div className="settings-inline">
        <button type="button" disabled={saving || !dirty} onClick={save}>
          Save folders
        </button>
        <button type="button" disabled={saving || (view.using_defaults && !dirty)} onClick={() => onSave(saveInput(null, view.disabled))}>
          Reset to defaults
        </button>
      </div>
    </div>
  );
}

/**
 * The Skills tab's container: reads the skills when the tab is first shown, and again on Refresh and after a save. There is no
 * polling: the folders change only when the user changes them.
 */
export function SkillsTab({ active }: { active: boolean }) {
  const [view, setView] = useState<SkillsView | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const seq = useRef(0);
  const applied = useRef(0);

  const read = () => {
    const my = ++seq.current;
    setLoading(true);
    api.listSkills().then(
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

  const save = (input: SkillsSettingsInput) => {
    setSaving(true);
    setSaveError(null);
    api.setSkillsSettings(input).then(
      () => {
        setSaving(false);
        read();
      },
      (e: unknown) => {
        setSaving(false);
        setSaveError(errorText(e));
      },
    );
  };

  useEffect(() => {
    if (active && view === null && loadError === null) {
      read();
    }
    // Read once, the first time the tab is shown. After that the tab reads again on Refresh and after a save.
  }, [active]);

  return (
    <SkillsSection
      view={view}
      loadError={loadError}
      loading={loading}
      saving={saving}
      saveError={saveError}
      onRefresh={read}
      onSave={save}
    />
  );
}
