import { useEffect, useId, useRef, useState } from 'react';
import { api, errorText } from '../lib/api';
import { capDisplay, displayText } from '../lib/keysDisplay';
import {
  FOLDER_STATUS_LABEL,
  LEVEL_LABEL,
  SOURCE_LABEL,
  removeFolder,
  saveInput,
  setOverride,
  setSwitch,
  shadowNote,
  withFolder,
} from '../lib/skillFolders';
import type { FolderStatus, SkillLevel, SkillsSettingsInput, SkillsView } from '../lib/types';
import { Sheet } from './Sheet';

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

/** The folder sheet: the row it edits (null for a new folder), what is typed, and whether a save was sent from it. */
interface FolderSheetState {
  index: number | null;
  draft: string;
  submitted: boolean;
}

/** The level a select's value names; null for anything else (the per-skill "global default"). */
function levelOf(value: string): SkillLevel | null {
  return value === 'ask' || value === 'auto' ? value : null;
}

/** The tag class of a folder's status: found is ok, a folder not there yet is muted, the rest need a look. */
const FOLDER_STATUS_CLASS: Record<FolderStatus, string> = {
  found: 'settings-tag settings-tag-ok',
  not_found: 'settings-tag settings-tag-muted',
  blocked: 'settings-tag settings-tag-fail',
  unresolved: 'settings-tag settings-tag-warn',
  duplicate: 'settings-tag settings-tag-warn',
};

/** "5 skills" or "5 skills · 1 off": the skills in use (a shadowed one is not counted) and how many are switched off. */
function skillCountText(view: SkillsView): string {
  const inUse = view.skills.filter((s) => s.shadowed_by === null);
  const off = inUse.filter((s) => !s.enabled).length;
  const count = `${inUse.length} ${inUse.length === 1 ? 'skill' : 'skills'}`;
  return off === 0 ? count : `${count} · ${off} off`;
}

/**
 * The Skills tab: how skills work, the folders Watt reads them from (the Watt folder is locked; the others are added and
 * edited in a sheet), the approval level, the skills found with a switch and a level each, and the warnings. A skill marked
 * hidden is in the list but never offered to the model. Every change is saved at once.
 */
export function SkillsSection({ view, loadError, loading, saving, saveError, onRefresh, onSave }: SkillsSectionProps) {
  const uid = useId();
  const [sheet, setSheet] = useState<FolderSheetState | null>(null);

  // A folder save that succeeds closes its sheet: the save is over, with no error. A failed one keeps the sheet open.
  useEffect(() => {
    if (sheet !== null && sheet.submitted && !saving && saveError === null) {
      setSheet(null);
    }
  }, [sheet, saving, saveError]);

  const closeSheet = () => {
    if (!saving) {
      setSheet(null);
    }
  };

  const submitFolder = (input: SkillsSettingsInput) => {
    setSheet((s) => (s === null ? s : { ...s, submitted: true }));
    onSave(input);
  };

  return (
    <section className="settings-section" aria-labelledby={`${uid}-title`}>
      <h3 className="settings-section-title" id={`${uid}-title`}>
        Skills
      </h3>
      <p className="settings-hint">
        A skill is a folder with a SKILL.md file. The model sees the skills and loads one with the skill tool when it fits the task. Folders
        starting with ~ are in your home folder, absolute paths are used as they are, and other relative paths are read from the folder of the
        current terminal session. A folder that is not there yet is kept.
      </p>

      {view !== null && (
        <div className="settings-card settings-approval">
          <div className="settings-inline">
            <label htmlFor={`${uid}-approval`}>Loading a skill</label>
            <select
              id={`${uid}-approval`}
              value={view.approval}
              disabled={saving}
              onChange={(e) => onSave(saveInput(savedFolders(view), view.disabled, { approval: levelOf(e.target.value) ?? view.approval, overrides: view.overrides }))}
            >
              <option value="ask">{LEVEL_LABEL.ask}</option>
              <option value="auto">{LEVEL_LABEL.auto}</option>
            </select>
          </div>
          <p className="settings-hint">
            Ask first: a card asks before a skill is loaded the first time in a turn. Applies to every skill unless one overrides it below.
            Approve all for the session still covers asks.
          </p>
        </div>
      )}

      {view !== null && (
        <SkillFolders
          view={view}
          saving={saving}
          onSave={onSave}
          onEdit={(index) => setSheet({ index, draft: index === null ? '' : view.folders[index].folder, submitted: false })}
        />
      )}

      <div className="settings-section-block">
        <div className="settings-card-head">
          <h4 className="settings-section-title">{view === null ? 'Skills' : skillCountText(view)}</h4>
          <button type="button" disabled={loading || saving} onClick={onRefresh}>
            {loading ? 'Refreshing...' : 'Refresh'}
          </button>
        </div>

        {saveError !== null && sheet === null && (
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
          <>
            <p className="settings-hint">No skills found in these folders yet.</p>
            {view.roots.length > 0 && (
              <>
                <p className="settings-hint">Searched:</p>
                <ul className="settings-hint-list">
                  {view.roots.map((r) => (
                    <li key={r.path}>
                      <code className="settings-truncate" title={displayText(r.path)}>
                        {displayText(r.path)}
                      </code>
                    </li>
                  ))}
                </ul>
              </>
            )}
          </>
        )}

        {view !== null && view.skills.length > 0 && (
          <ul className="settings-cards">
            {view.skills.map((skill) => {
              const hidden = skill.shadowed_by !== null;
              const name = displayText(skill.name);
              const cls = hidden
                ? 'settings-card settings-card-muted settings-skill settings-skill-muted'
                : skill.enabled
                  ? 'settings-card settings-skill'
                  : 'settings-card settings-skill is-off';
              return (
                <li key={skill.path} className={cls}>
                  <div className="settings-skill-main">
                    <div className="settings-skill-title">
                      <span className="settings-row-name">{name}</span>
                      <span className="settings-tag settings-tag-muted">{SOURCE_LABEL[skill.source]}</span>
                      {!hidden && !skill.enabled && <span className="settings-tag settings-tag-muted">switched off</span>}
                      {!skill.model_invocable && <span className="settings-tag settings-tag-warn">hidden from the model</span>}
                      {hidden && <span className="settings-tag settings-tag-muted">shadowed</span>}
                    </div>
                    {hidden && <span className="settings-dim">{displayText(shadowNote(skill.shadowed_by ?? ''))}</span>}
                    <p className="settings-dim settings-clamp" title={displayText(skill.description)}>
                      {capDisplay(skill.description, SKILL_DESCRIPTION_CHARS)}
                    </p>
                    <span className="settings-dim settings-truncate" title={displayText(skill.path)}>
                      {displayText(skill.path)}
                    </span>
                  </div>
                  {skill.source === 'builtin' && (
                    <div className="settings-skill-controls">
                      <span className="settings-tag settings-tag-muted" title="Always available">
                        locked
                      </span>
                    </div>
                  )}
                  {!hidden && skill.source !== 'builtin' && (
                    <div className="settings-skill-controls">
                      <label className="settings-check">
                        <input
                          type="checkbox"
                          checked={skill.enabled}
                          disabled={saving}
                          onChange={(e) => onSave(saveInput(savedFolders(view), setSwitch(view.disabled, skill.name, e.target.checked), view))}
                        />
                        Use
                      </label>
                      <select
                        aria-label={`Loading ${name}`}
                        value={skill.override ?? 'global'}
                        disabled={saving || !skill.enabled}
                        onChange={(e) =>
                          onSave(saveInput(savedFolders(view), view.disabled, { approval: view.approval, overrides: setOverride(view.overrides, skill.name, levelOf(e.target.value)) }))
                        }
                      >
                        <option value="global">Global default ({LEVEL_LABEL[view.approval]})</option>
                        <option value="ask">{LEVEL_LABEL.ask}</option>
                        <option value="auto">{LEVEL_LABEL.auto}</option>
                      </select>
                    </div>
                  )}
                </li>
              );
            })}
          </ul>
        )}
      </div>

      {view !== null && sheet !== null && (
        <FolderSheet
          view={view}
          sheet={sheet}
          saving={saving}
          saveError={saveError}
          onDraft={(draft) => setSheet({ ...sheet, draft, submitted: false })}
          onSubmit={submitFolder}
          onClose={closeSheet}
        />
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

/** The folder settings as they are saved: the folders as configured, or null while the defaults apply. */
function savedFolders(view: SkillsView): string[] | null {
  return view.using_defaults ? null : view.folders.map((f) => f.folder);
}

/**
 * The folder list: the locked Watt folder, then the configured folders, each as a card with its source, its status, the text
 * as configured and (when it differs) the resolved path. Add and Edit open the folder sheet; Remove and Reset save at once.
 */
function SkillFolders({ view, saving, onSave, onEdit }: { view: SkillsView; saving: boolean; onSave(input: SkillsSettingsInput): void; onEdit(index: number | null): void }) {
  const watt = view.roots.find((r) => r.source === 'watt');
  const rows = view.folders.map((f) => f.folder);
  const sourceOf = (path: string | null) => (path === null ? undefined : view.roots.find((r) => r.path === path)?.source);
  return (
    <div className="settings-section-block">
      <div className="settings-card-head">
        <h4 className="settings-section-title">Skill folders</h4>
        <div className="settings-card-actions">
          <button type="button" disabled={saving} onClick={() => onEdit(null)}>
            Add folder
          </button>
          <button type="button" disabled={saving || view.using_defaults} onClick={() => onSave(saveInput(null, view.disabled, view))}>
            Reset to defaults
          </button>
        </div>
      </div>
      <ul className="settings-cards">
        {watt !== undefined && (
          <li className="settings-card skill-folder">
            <div className="settings-card-head">
              <span className="settings-tag settings-tag-muted">{SOURCE_LABEL.watt}</span>
              <span className="settings-tags">
                <span className={watt.exists ? 'settings-tag settings-tag-ok' : 'settings-tag settings-tag-muted'}>{watt.exists ? 'found' : 'not found'}</span>
                <span className="settings-tag settings-tag-muted">locked</span>
              </span>
            </div>
            <code className="settings-truncate" title={displayText(watt.path)}>
              {displayText(watt.path)}
            </code>
          </li>
        )}
        {view.folders.map((entry, i) => {
          const source = sourceOf(entry.path);
          const label = displayText(entry.folder);
          return (
            <li key={`${i}-${entry.folder}`} className="settings-card skill-folder">
              <div className="settings-card-head">
                <span className="settings-tags">{source !== undefined && <span className="settings-tag settings-tag-muted">{SOURCE_LABEL[source]}</span>}</span>
                <span className={FOLDER_STATUS_CLASS[entry.status]}>{FOLDER_STATUS_LABEL[entry.status]}</span>
              </div>
              <code className="settings-truncate" title={label}>
                {label}
              </code>
              {entry.path !== null && entry.path !== entry.folder && <span className="settings-path">{displayText(entry.path)}</span>}
              <div className="settings-card-actions">
                <button type="button" disabled={saving} aria-label={`Edit ${label}`} onClick={() => onEdit(i)}>
                  Edit
                </button>
                <button type="button" disabled={saving} aria-label={`Remove ${label}`} onClick={() => onSave(saveInput(removeFolder(rows, i), view.disabled, view))}>
                  Remove
                </button>
              </div>
            </li>
          );
        })}
      </ul>
    </div>
  );
}

/** The sheet that adds or edits one folder. The folder is checked as it is typed; Save persists it at once. */
export function FolderSheet({
  view,
  sheet,
  saving,
  saveError,
  onDraft,
  onSubmit,
  onClose,
}: {
  view: SkillsView;
  sheet: FolderSheetState;
  saving: boolean;
  saveError: string | null;
  onDraft(draft: string): void;
  onSubmit(input: SkillsSettingsInput): void;
  onClose(): void;
}) {
  const uid = useId();
  const inputRef = useRef<HTMLInputElement>(null);
  const [picking, setPicking] = useState(false);
  const [pickError, setPickError] = useState<string | null>(null);
  const editing = sheet.index !== null;
  const rows = view.folders.map((f) => f.folder);
  const result = withFolder(rows, sheet.index, sheet.draft);
  const trimmed = sheet.draft.trim();
  const unchanged = sheet.index !== null && trimmed === rows[sheet.index];
  const canSave = !saving && trimmed !== '' && result.error === null && !unchanged;
  const save = () => {
    if (canSave) {
      onSubmit(saveInput(result.rows, view.disabled, view));
    }
  };
  // The native picker is modal, so the sheet just waits for it. Its input gets focus back once it closes, whatever it returned.
  const browse = async () => {
    const current = sheet.draft.trim();
    setPicking(true);
    setPickError(null);
    try {
      const picked = await api.pickFolder(current !== '' && !current.startsWith('~') ? current : undefined);
      if (picked !== null) {
        onDraft(picked);
      }
    } catch (e) {
      setPickError(errorText(e));
    } finally {
      setPicking(false);
      inputRef.current?.focus();
    }
  };
  return (
    <Sheet
      title={editing ? 'Skill folder' : 'Add skill folder'}
      onClose={onClose}
      footer={
        <>
          <button type="button" disabled={saving} onClick={onClose}>
            Cancel
          </button>
          <button type="button" className="settings-primary" disabled={!canSave} onClick={save}>
            {editing ? 'Save folder' : 'Add folder'}
          </button>
        </>
      }
    >
      <div className="settings-inline">
        <input
          ref={inputRef}
          data-autofocus
          aria-label="Skill folder"
          aria-invalid={result.error !== null || undefined}
          aria-describedby={`${uid}-hint`}
          placeholder="~/my-skills or a path"
          value={sheet.draft}
          readOnly={saving}
          spellCheck={false}
          onChange={(e) => onDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault();
              save();
            }
          }}
        />
        <button type="button" disabled={saving || picking} onClick={browse}>
          Browse…
        </button>
      </div>
      {pickError !== null && (
        <span className="settings-error" role="alert">
          {pickError}
        </span>
      )}
      {result.error !== null && (
        <span className="settings-error" role="alert">
          {result.error}
        </span>
      )}
      {sheet.submitted && saveError !== null && (
        <span className="settings-error" role="alert">
          {saveError}
        </span>
      )}
      <ul className="settings-hint-list" id={`${uid}-hint`}>
        <li>~/... is in your home folder.</li>
        <li>An absolute path (C:\... or /...) is used as it is.</li>
        <li>Any other relative path is read from the folder of the terminal session.</li>
        <li>A folder that is not there yet is kept.</li>
      </ul>
    </Sheet>
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
