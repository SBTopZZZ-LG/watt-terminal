import { useCallback, useEffect, useId, useRef, useState } from 'react';
import type { KeyboardEvent } from 'react';
import { api, errorText } from '../lib/api';
import {
  barTone,
  loaderText,
  NO_SETTLED,
  pollDelay,
  serverTone,
  settleServers,
  showsLoader,
  stateLabel,
  stateMessage,
  toggleTip,
  unsettleServer,
  withEnabled,
} from '../lib/mcpBar';
import type { Settled } from '../lib/mcpBar';
import type { McpServerStatus, McpStatus } from '../lib/types';
import { BrailleSpinner } from './BrailleSpinner';

/**
 * The status, read now and again on the poll delay (see pollDelay). It is not read while the tab is hidden; it is read once
 * when the tab becomes visible again. A failed read keeps the last status. `setStatus` is for an optimistic change; the next
 * read replaces it. `settled` holds the servers that have settled this session; `forget` removes one (the user switched it on).
 */
export function useMcpStatus(open: boolean) {
  const [status, setStatus] = useState<McpStatus | null>(null);
  const [settled, setSettled] = useState<Settled>(NO_SETTLED);
  // Bumped when a read ends, ok or not, so the poll timer is set again after each one.
  const [reads, setReads] = useState(0);
  const refresh = useCallback(() => {
    api
      .getMcpStatus()
      .then(
        (next) => {
          setStatus(next);
          setSettled((s) => settleServers(s, next.servers));
        },
        () => undefined,
      )
      .finally(() => setReads((n) => n + 1));
  }, []);
  const forget = useCallback((name: string) => setSettled((s) => unsettleServer(s, name)), []);
  useEffect(() => {
    refresh();
  }, [refresh]);
  const delay = pollDelay(status, open);
  useEffect(() => {
    if (delay === null) {
      return;
    }
    const timer = setTimeout(() => {
      // A hidden tab stops here; the visibility listener reads again when it shows.
      if (!document.hidden) {
        refresh();
      }
    }, delay);
    return () => clearTimeout(timer);
  }, [reads, delay, refresh]);
  useEffect(() => {
    const onVisible = () => {
      if (!document.hidden) {
        refresh();
      }
    };
    document.addEventListener('visibilitychange', onVisible);
    return () => document.removeEventListener('visibilitychange', onVisible);
  }, [refresh]);
  return { status, setStatus, refresh, settled, forget };
}

/** The loader row above the composer, while an enabled server is connecting. Nothing otherwise. */
export function McpLoader({ starting, servers, settled = NO_SETTLED }: { starting: boolean; servers: McpServerStatus[]; settled?: Settled }) {
  if (!starting) {
    return null;
  }
  return (
    <div className="bar-mcp-loader" role="status" aria-live="polite">
      <BrailleSpinner />
      <span>{loaderText(servers, settled)}</span>
    </div>
  );
}

interface PanelProps {
  id: string;
  open: boolean;
  servers: McpServerStatus[];
  /** The name of the server whose switch is being saved. All switches wait for it. */
  pending: string | null;
  /** The last switch that failed, or null. */
  error: string | null;
  onToggle: (server: McpServerStatus) => void;
}

/** The panel's contents: one row per server, with its state and its switch. Always in the DOM, hidden while closed. */
export function McpPanel({ id, open, servers, pending, error, onToggle }: PanelProps) {
  return (
    <div id={id} className="mcp-panel" role="region" aria-label="MCP servers" hidden={!open}>
      <p className="mcp-panel-head">MCP servers</p>
      {servers.length === 0 ? (
        <p className="mcp-panel-empty">No MCP servers yet.</p>
      ) : (
        <ul className="mcp-panel-rows">
          {servers.map((server) => (
            <li key={server.name} className="mcp-row">
              <span className="mcp-dot" data-tone={serverTone(server)} aria-hidden="true" />
              <span className="mcp-row-name">{server.name}</span>
              <span className="mcp-row-state" title={stateMessage(server)}>
                {stateLabel(server)}
              </span>
              <button
                type="button"
                role="switch"
                className="mcp-switch"
                aria-checked={server.enabled}
                aria-label={server.name}
                data-tip={toggleTip(server.enabled)}
                disabled={pending !== null}
                onClick={() => onToggle(server)}
              >
                <span className="mcp-switch-knob" aria-hidden="true" />
              </button>
            </li>
          ))}
        </ul>
      )}
      {error !== null && (
        <p className="mcp-panel-error" role="alert">
          {error}
        </p>
      )}
      <p className="mcp-panel-foot">Manage servers in Settings &gt; MCP.</p>
      <p className="mcp-panel-foot">{'Changes take effect on the agent’s next step.'}</p>
    </div>
  );
}

/**
 * The composer's MCP control, next to the attach button: the loader row while servers start, the icon button with its status
 * dot, and the panel above it. The panel opens on click, closes on Escape (focus back to the button) or a press outside, and
 * re-reads the status each time it opens. Its switches save through api.mcpSetEnabled.
 */
export function McpBar() {
  const [open, setOpen] = useState(false);
  const [pending, setPending] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const { status, setStatus, refresh, settled, forget } = useMcpStatus(open);
  const wrapRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const panelId = useId();
  const servers = status?.servers ?? [];
  const tone = barTone(servers);

  useEffect(() => {
    if (!open) {
      return;
    }
    const onPress = (e: MouseEvent) => {
      if (!wrapRef.current?.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener('mousedown', onPress);
    return () => document.removeEventListener('mousedown', onPress);
  }, [open]);

  const toggleOpen = () => {
    if (!open) {
      refresh();
    }
    setOpen(!open);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === 'Escape' && open) {
      e.preventDefault();
      e.stopPropagation();
      setOpen(false);
      buttonRef.current?.focus();
    }
  };

  const toggle = async (server: McpServerStatus) => {
    const next = !server.enabled;
    setPending(server.name);
    setError(null);
    if (next) {
      // Switching on shows the loader once for this connect, even if the server settled earlier this session.
      forget(server.name);
    }
    setStatus((s) => s && withEnabled(s, server.name, next));
    try {
      await api.mcpSetEnabled(server.name, next);
    } catch (e) {
      setStatus((s) => s && withEnabled(s, server.name, server.enabled));
      setError(`${server.name}: ${errorText(e)}`);
    } finally {
      setPending(null);
      refresh();
    }
  };

  return (
    <div ref={wrapRef} className="bar-mcp" onKeyDown={onKeyDown}>
      <McpLoader starting={showsLoader(servers, settled)} servers={servers} settled={settled} />
      <McpPanel id={panelId} open={open} servers={servers} pending={pending} error={error} onToggle={toggle} />
      <button
        ref={buttonRef}
        type="button"
        className="bar-mcp-button"
        aria-label="MCP servers"
        aria-expanded={open}
        aria-controls={panelId}
        data-tip="MCP servers"
        onClick={toggleOpen}
      >
        <PlugIcon />
        {tone !== 'off' && <span className="mcp-dot" data-tone={tone} aria-hidden="true" />}
      </button>
    </div>
  );
}

function PlugIcon() {
  return (
    <svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true" focusable="false">
      <path
        d="M9 3v5M15 3v5M6 8h12v4a6 6 0 0 1-12 0zM12 18v3"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}
