import type { FolderStatus, SkillLevel, SkillView, SkillsSettingsInput } from './types';

/** Most folders the settings may hold (core/skills/folders.rs MAX_FOLDERS). */
export const MAX_SKILL_FOLDERS = 64;
/** Longest folder setting, in characters (core/skills/folders.rs MAX_FOLDER_CHARS). */
export const MAX_SKILL_FOLDER_CHARS = 1024;

/** The badge that says where a skill comes from. */
export const SOURCE_LABEL: Record<SkillView['source'], string> = {
  watt: 'watt config',
  user: 'your folder',
  project: 'this project',
};

/** The short status of a configured folder. */
export const FOLDER_STATUS_LABEL: Record<FolderStatus, string> = {
  found: 'found',
  not_found: 'not found',
  blocked: 'blocked',
  unresolved: 'needs a project or home folder',
  duplicate: 'same folder as an earlier row',
};

/** Why a folder setting cannot be saved, or null. The Rust side checks the same things again on save. */
export function folderProblem(folder: string): string | null {
  if (folder.trim() === '') return 'A folder is empty.';
  if (folder.length > MAX_SKILL_FOLDER_CHARS) return `A folder is longer than ${MAX_SKILL_FOLDER_CHARS} characters.`;
  if (folder.includes('\0')) return 'A folder has a NUL character.';
  return null;
}

/** The first problem with any row, or null. */
export function rowsProblem(rows: string[]): string | null {
  if (rows.length > MAX_SKILL_FOLDERS) return `At most ${MAX_SKILL_FOLDERS} folders.`;
  for (const row of rows) {
    const problem = folderProblem(row);
    if (problem !== null) return problem;
  }
  return null;
}

/** Adds a folder at the end. Returns the rows and an error (rows unchanged) when the folder cannot be added. */
export function addFolder(rows: string[], raw: string): { rows: string[]; error: string | null } {
  const folder = raw.trim();
  const problem = folderProblem(folder);
  if (problem !== null) return { rows, error: problem };
  if (rows.length >= MAX_SKILL_FOLDERS) return { rows, error: `At most ${MAX_SKILL_FOLDERS} folders.` };
  if (rows.includes(folder)) return { rows, error: 'That folder is already in the list.' };
  return { rows: [...rows, folder], error: null };
}

/** The rows without the one at `index`. */
export function removeFolder(rows: string[], index: number): string[] {
  return rows.filter((_, i) => i !== index);
}

/** The rows with the one at `index` replaced by `value` (saved trimmed). */
export function setFolder(rows: string[], index: number, value: string): string[] {
  return rows.map((row, i) => (i === index ? value : row));
}

/** The switched-off names with `name` turned on (removed) or off (added once). Order is kept. */
export function setSwitch(disabled: string[], name: string, on: boolean): string[] {
  if (on) return disabled.filter((n) => n !== name);
  return disabled.includes(name) ? disabled : [...disabled, name];
}

/** The approval settings a save sends. A save of folders or switches passes the view's, so it keeps them unchanged. */
export type Approvals = Pick<SkillsSettingsInput, 'approval' | 'overrides'>;

/** The labels of the approval levels, for the global setting and the per-skill setting. */
export const LEVEL_LABEL: Record<SkillLevel, string> = { ask: 'Ask first', auto: "Don't ask" };

/** What to send for the rows, the switches and the approvals. Folders are trimmed; null (the defaults) when `rows` is null. */
export function saveInput(rows: string[] | null, disabled: string[], approvals: Approvals): SkillsSettingsInput {
  return {
    folders: rows === null ? null : rows.map((row) => row.trim()),
    disabled,
    approval: approvals.approval,
    overrides: approvals.overrides,
  };
}

/** The per-skill levels with `name` set to `level`. null removes its entry: the global level applies again. */
export function setOverride(overrides: Record<string, SkillLevel>, name: string, level: SkillLevel | null): Record<string, SkillLevel> {
  const next = { ...overrides };
  if (level === null) {
    delete next[name];
  } else {
    next[name] = level;
  }
  return next;
}

/** The note on a shadowed skill: the folder of the one in use. */
export function shadowNote(folder: string): string {
  return `Hidden: the one in ${folder} is used instead.`;
}
