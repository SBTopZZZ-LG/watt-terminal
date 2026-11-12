// The context meter's numbers and words. Pure, so the component and its tests read a ContextUsage the same way.
// The categories and their order come from the backend (core/chat/context.rs); this file only labels and measures them.

import type { ContextUsage } from './types';
import { safeText } from './historyView';

/** The labels of the categories the backend sends. Any other id is shown as its own (sanitised) text. */
const LABELS: Record<string, string> = {
  system: 'System instructions',
  tools: 'Tool definitions',
  user: 'Your messages',
  attachments: 'Attachments',
  assistant: 'Agent messages',
  tool_output: 'Tool output',
  other: 'Other',
};

/** ok below 70%, warn from 70%, high from 85% (or when the reply's reserve would not fit). Unknown when no limit is known. */
export type MeterLevel = 'unknown' | 'ok' | 'warn' | 'high';

/** A row of the breakdown. Categories with no tokens are left out; Free and Reserved come only when the limit is known. */
export interface MeterRow {
  kind: 'category' | 'free' | 'reserved';
  id: string;
  label: string;
  tokens: number;
  /** Of the limit when known, else of the tokens used. A label: '37%', '<1%' or '0%'. */
  percent: string;
  /** The category's place among the category rows (its colour), or null for Free and Reserved. */
  slot: number | null;
}

/** One part of the stacked bar: its share of the limit (of the tokens used when the limit is not known), in percent. */
export interface Segment {
  id: string;
  label: string;
  share: number;
}

/** The expanded breakdown is at most this tall, and at most 60% of the agent panel. */
export const DETAIL_MAX_PX = 240;
const DETAIL_SHARE = 0.6;

/** A limit the meter can measure against: known and positive. */
function knownLimit(u: Pick<ContextUsage, 'limit'>): number | null {
  return u.limit !== null && u.limit > 0 ? u.limit : null;
}

/**
 * A token count for the collapsed line and the rows: '850', '1.2k', '48k', '128k', '1.2M'. One decimal under 10 of a unit,
 * whole numbers from 10 up. Negative and non-finite values count as 0.
 */
export function formatTokens(n: number): string {
  const v = Number.isFinite(n) ? Math.max(0, Math.round(n)) : 0;
  if (v < 1000) {
    return String(v);
  }
  if (Math.round(v / 1000) < 1000) {
    return `${compact(v / 1000)}k`;
  }
  return `${compact(v / 1_000_000)}M`;
}

function compact(x: number): string {
  const text = x < 10 ? (Math.round(x * 10) / 10).toFixed(1) : String(Math.round(x));
  return text.endsWith('.0') ? text.slice(0, -2) : text;
}

/** A whole count with thousands separators, for the headline: 48213 -> '48,213'. */
export function groupDigits(n: number): string {
  const v = Number.isFinite(n) ? Math.max(0, Math.round(n)) : 0;
  return String(v).replace(/\B(?=(\d{3})+(?!\d))/g, ',');
}

/** The part as a percentage of the whole, not rounded. 0 when the whole is not positive. */
export function share(part: number, whole: number): number {
  return whole > 0 ? (part / whole) * 100 : 0;
}

/** A percentage label: '0%' for nothing, '<1%' for a little, else the rounded whole percent ('37%'). */
export function percent(part: number, whole: number): string {
  if (part <= 0 || whole <= 0) {
    return '0%';
  }
  const s = share(part, whole);
  return s < 0.5 ? '<1%' : `${Math.round(s)}%`;
}

/**
 * How full the context is. Compared with integers, so 70 and 85 are exact. High also when the used tokens plus the reserve
 * for the reply do not fit the limit, even if the used part alone is below 85%.
 */
export function level(u: Pick<ContextUsage, 'limit' | 'used' | 'reserved_output'>): MeterLevel {
  const limit = knownLimit(u);
  if (limit === null) {
    return 'unknown';
  }
  if (u.used + u.reserved_output > limit) {
    return 'high';
  }
  if (u.used * 100 >= 85 * limit) {
    return 'high';
  }
  if (u.used * 100 >= 70 * limit) {
    return 'warn';
  }
  return 'ok';
}

/**
 * The name of a category. An id the app does not know is shown as its text, with control characters and invisible format
 * characters taken out. Own properties only, so an id such as 'constructor' is not mistaken for a label.
 */
export function labelOf(id: string): string {
  return Object.prototype.hasOwnProperty.call(LABELS, id) ? LABELS[id] : safeText(id);
}

/** The headline of the expanded view: '48,213 of 128,000 tokens (37%)', or without the limit: '48,213 tokens'. */
export function headline(u: ContextUsage): string {
  const limit = knownLimit(u);
  if (limit === null) {
    return `Context: ${groupDigits(u.used)} tokens (limit unknown)`;
  }
  return `Context: ${groupDigits(u.used)} of ${groupDigits(limit)} tokens (${percent(u.used, limit)})`;
}

/** The categories with tokens, in the backend's order. Their index is the colour slot the bar and the rows share. */
function shownCategories(u: ContextUsage): { id: string; tokens: number }[] {
  return u.categories.filter((c) => c.tokens > 0);
}

/** The breakdown rows: the categories, then (limit known) Free and Reserved for the reply. */
export function rows(u: ContextUsage): MeterRow[] {
  const limit = knownLimit(u);
  const whole = limit ?? u.used;
  const out: MeterRow[] = shownCategories(u).map((c, slot) => ({
    kind: 'category',
    id: c.id,
    label: labelOf(c.id),
    tokens: c.tokens,
    percent: percent(c.tokens, whole),
    slot,
  }));
  if (limit !== null) {
    const free = Math.max(0, limit - u.used - u.reserved_output);
    out.push({ kind: 'free', id: 'free', label: 'Free', tokens: free, percent: percent(free, limit), slot: null });
    out.push({
      kind: 'reserved',
      id: 'reserved',
      label: 'Reserved for reply',
      tokens: u.reserved_output,
      percent: percent(u.reserved_output, limit),
      slot: null,
    });
  }
  return out;
}

/**
 * The stacked bar: one segment per shown category, in the same order as the rows. Each share is of the limit, or of the
 * tokens used when no limit is known. When the shares add up to more than 100 (used above the limit), they are scaled down.
 */
export function segments(u: ContextUsage): Segment[] {
  const limit = knownLimit(u);
  const whole = limit ?? u.used;
  const parts: Segment[] = shownCategories(u).map((c) => ({ id: c.id, label: labelOf(c.id), share: share(c.tokens, whole) }));
  const total = parts.reduce((sum, p) => sum + p.share, 0);
  if (total > 100) {
    const scale = 100 / total;
    for (const p of parts) {
      p.share *= scale;
    }
  }
  return parts;
}

/** The expanded view's height cap for an agent panel of this height: 60% of it, at most 240px. Unmeasured (0) gives 240px. */
export function detailMaxHeight(panelPx: number): number {
  if (!(panelPx > 0)) {
    return DETAIL_MAX_PX;
  }
  return Math.min(DETAIL_MAX_PX, Math.floor(panelPx * DETAIL_SHARE));
}
