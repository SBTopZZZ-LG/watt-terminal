import { useEffect, useId, useRef, useState } from 'react';
import { api, errorText } from '../lib/api';
import { capDisplay, displayText } from '../lib/keysDisplay';
import { defaultOptionLabel, draftFromServer, LEVEL_LABEL, overrideFromValue, overrideValue, stateMessage, stateTag, transportSummary } from '../lib/mcpForm';
import type { McpImportResult, McpLevel, McpServerInput, McpServerView, McpToolView, McpView } from '../lib/types';
import { McpAddPanel, McpServerForm } from './McpServerForm';

/** How often the tab re-reads the servers while it is open. The backend emits no MCP events, so this is the only refresh. */
export const MCP_POLL_MS = 2000;
/** The most log lines shown; the newest ones. */
export const MCP_LOG_LINES = 200;

/** The panel under the list: adding a server, or editing one by name. */
export type McpPanel = { kind: 'add' } | { kind: 'edit'; name: string };

/** The open server's log: its lines once loaded (null while loading), and the last error. */
export interface McpLogsState {
  name: string;
  lines: string[] | null;
  error: string | null;
}

export interface McpSectionProps {
  view: McpView | null;
  loadError: string | null;
  actionError: string | null;
  /** A change is in flight: every control that changes a server is off. */
  busy: boolean;
  panel: McpPanel | null;
  logs: McpLogsState | null;
  /** The server whose Remove is waiting for its confirm step. */
  askRemove: string | null;
  onToggleEnabled(name: string, enabled: boolean): void;
  onServerApproval(name: string, level: McpLevel): void;
  onToolApproval(name: string, tool: string, level: McpLevel | null): void;
  onRestart(name: string): void;
  onToggleLogs(name: string): void;
  onAskRemove(name: string | null): void;
  onRemove(name: string): void;
  onEdit(name: string): void;
  onOpenAdd(): void;
  onClosePanel(): void;
  onSave(input: McpServerInput): Promise<void>;
  onImport(text: string): Promise<McpImportResult>;
}

/**
 * The MCP section of the settings: the servers, each with its state, enable switch, approval, restart, logs, edit and remove,
 * and the tools of each with their own approval. Adding a server or pasting a list opens a panel under the list. Changes are
 * saved at once (no Save for the section); the view each change returns replaces the one shown.
 */
export function McpSection(p: McpSectionProps) {
  const uid = useId();
  const servers = p.view?.servers ?? [];
  const editName = p.panel?.kind === 'edit' ? p.panel.name : null;
  const editing = editName === null ? undefined : servers.find((s) => s.config.name === editName);
  return (
    <section className="settings-section" aria-labelledby={`${uid}-title`}>
      <h3 className="settings-section-title" id={`${uid}-title`}>
        MCP servers
      </h3>
      <p className="settings-hint">
        A server's tools are offered to the model as mcp__server__tool. Each tool asks first unless its approval says otherwise; global
        approve-all skips these prompts too. Secret values are stored in the secret store and never shown again.
      </p>
      {p.loadError !== null && (
        <p className="settings-error" role="alert">
          {p.loadError}
        </p>
      )}
      {p.actionError !== null && (
        <p className="settings-error" role="alert">
          {p.actionError}
        </p>
      )}
      {p.view === null && p.loadError === null && <p className="settings-hint">Loading MCP servers...</p>}
      {p.view !== null && servers.length === 0 && <p className="settings-hint">No MCP servers yet. Add one below.</p>}

      <ul className="settings-mcp-list">
        {servers.map((server) => (
          <McpServerRow key={server.config.name} {...p} server={server} />
        ))}
      </ul>

      {p.panel?.kind === 'add' && (
        <McpAddPanel busy={p.busy} takenNames={servers.map((s) => s.config.name)} onSave={p.onSave} onImport={p.onImport} onClose={p.onClosePanel} />
      )}
      {editing !== undefined && (
        <section className="settings-mcp-panel" aria-label={`Edit ${editing.config.name}`}>
          <McpServerForm
            key={`edit-${editing.config.name}`}
            initial={draftFromServer(editing)}
            original={editing.config}
            takenNames={servers.map((s) => s.config.name)}
            busy={p.busy}
            onSave={p.onSave}
            onCancel={p.onClosePanel}
          />
        </section>
      )}
      {p.panel === null && (
        <div className="settings-inline">
          <button type="button" disabled={p.busy || p.view === null} onClick={p.onOpenAdd}>
            Add server
          </button>
        </div>
      )}
    </section>
  );
}

type RowProps = McpSectionProps & { server: McpServerView };

/** One server: its line, its controls, its error, its logs and its tools. */
function McpServerRow(p: RowProps) {
  const { server } = p;
  const config = server.config;
  const name = config.name;
  const [toolsOpen, setToolsOpen] = useState(false);
  const tag = stateTag(server.state);
  const message = stateMessage(server.state);
  const logsOpen = p.logs !== null && p.logs.name === name;
  const tools = server.tools;
  return (
    <li className="settings-mcp">
      <div className="settings-inline">
        <span className="settings-row-name">{name}</span>
        <span className={`settings-tag settings-tag-${tag.tone}`}>{tag.label}</span>
        {!config.enabled && <span className="settings-tag settings-tag-muted">off</span>}
      </div>
      <span className="settings-dim">{displayText(transportSummary(config.transport))}</span>
      {message !== null && (
        <span className="settings-error" role="status">
          {message}
        </span>
      )}
      <div className="settings-inline">
        <label className="settings-check">
          <input type="checkbox" checked={config.enabled} disabled={p.busy} onChange={(e) => p.onToggleEnabled(name, e.target.checked)} />
          Enabled
        </label>
        <label className="settings-inline-label">
          Approval
          <select value={config.approval} disabled={p.busy} onChange={(e) => p.onServerApproval(name, e.target.value as McpLevel)}>
            {(Object.keys(LEVEL_LABEL) as McpLevel[]).map((level) => (
              <option key={level} value={level}>
                {LEVEL_LABEL[level]}
              </option>
            ))}
          </select>
        </label>
        <button type="button" disabled={p.busy || !config.enabled} onClick={() => p.onRestart(name)}>
          Restart
        </button>
        <button type="button" aria-expanded={logsOpen} onClick={() => p.onToggleLogs(name)}>
          Logs
        </button>
        <button type="button" disabled={p.busy} onClick={() => p.onEdit(name)}>
          Edit
        </button>
        {p.askRemove === name ? (
          <span className="settings-inline">
            <span className="settings-warn">Remove {name} and its stored values?</span>
            <button type="button" className="settings-danger" disabled={p.busy} onClick={() => p.onRemove(name)}>
              Yes, remove
            </button>
            <button type="button" onClick={() => p.onAskRemove(null)}>
              No
            </button>
          </span>
        ) : (
          <button type="button" className="settings-danger" disabled={p.busy} onClick={() => p.onAskRemove(name)}>
            Remove
          </button>
        )}
      </div>

      <div className="settings-inline">
        <button type="button" aria-expanded={toolsOpen} onClick={() => setToolsOpen((open) => !open)}>
          {`Tools (${tools.length})`}
        </button>
      </div>
      {toolsOpen && <McpToolList name={name} tools={tools} busy={p.busy} onToolApproval={p.onToolApproval} />}

      {logsOpen && p.logs !== null && <McpLogs logs={p.logs} />}
    </li>
  );
}

/** A server's tools, each with its approval select. `name` is the server's name, which the change needs. */
export function McpToolList({
  name,
  tools,
  busy,
  onToolApproval,
}: {
  name: string;
  tools: McpToolView[];
  busy: boolean;
  onToolApproval(name: string, tool: string, level: McpLevel | null): void;
}) {
  if (tools.length === 0) {
    return <p className="settings-hint">No tools listed. A server that is off, failed or still starting lists none.</p>;
  }
  return (
    <ul className="settings-mcp-tools">
      {tools.map((tool) => (
        <li key={tool.name} className="settings-mcp-tool">
          <code>{displayText(tool.name)}</code>
          <span className="settings-dim">{capDisplay(tool.description, 160)}</span>
          <select
            aria-label={`Approval for ${tool.name}`}
            value={overrideValue(tool.override)}
            disabled={busy}
            onChange={(e) => onToolApproval(name, tool.name, overrideFromValue(e.target.value))}
          >
            <option value="">{defaultOptionLabel(tool.effective)}</option>
            <option value="ask">{LEVEL_LABEL.ask}</option>
            <option value="auto">{LEVEL_LABEL.auto}</option>
            <option value="never">{LEVEL_LABEL.never}</option>
          </select>
        </li>
      ))}
    </ul>
  );
}

function McpLogs({ logs }: { logs: McpLogsState }) {
  if (logs.error !== null) {
    return (
      <p className="settings-error" role="alert">
        {logs.error}
      </p>
    );
  }
  if (logs.lines === null) {
    return <p className="settings-hint">Loading logs...</p>;
  }
  const shown = logs.lines.slice(-MCP_LOG_LINES);
  return <pre className="settings-logs">{shown.length === 0 ? '(no output)' : shown.map((l) => displayText(l)).join('\n')}</pre>;
}

/**
 * The MCP tab's container: loads the servers when the tab is shown, re-reads them every MCP_POLL_MS while it is shown (and the
 * open log with them), and applies each change's returned view. A poll is skipped while a change is in flight. Responses are
 * applied in the order they were asked, so a poll that started before a change cannot overwrite the change's view.
 */
export function McpTab({ active }: { active: boolean }) {
  const [view, setView] = useState<McpView | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [pending, setPending] = useState(0);
  const [panel, setPanel] = useState<McpPanel | null>(null);
  const [logs, setLogs] = useState<McpLogsState | null>(null);
  const [askRemove, setAskRemove] = useState<string | null>(null);
  const seq = useRef(0);
  const applied = useRef(0);
  const pendingRef = useRef(0);
  const logsName = logs?.name ?? null;

  useEffect(() => {
    if (!active) {
      return;
    }
    let live = true;
    const refresh = () => {
      if (pendingRef.current > 0) {
        return;
      }
      const my = ++seq.current;
      api.getMcp().then(
        (v) => {
          if (live && my > applied.current) {
            applied.current = my;
            setView(v);
            setLoadError(null);
          }
        },
        (e: unknown) => {
          if (live) {
            setLoadError(errorText(e));
          }
        },
      );
      if (logsName !== null) {
        api.mcpLogs(logsName).then(
          (lines) => {
            if (live) {
              setLogs((prev) => (prev?.name === logsName ? { name: logsName, lines, error: null } : prev));
            }
          },
          (e: unknown) => {
            if (live) {
              setLogs((prev) => (prev?.name === logsName ? { ...prev, error: errorText(e) } : prev));
            }
          },
        );
      }
    };
    refresh();
    const timer = window.setInterval(refresh, MCP_POLL_MS);
    return () => {
      live = false;
      window.clearInterval(timer);
    };
  }, [active, logsName]);

  /** Runs one change: marks it in flight, applies the view it returns, then `done`. Rejects with the error. */
  const track = <T,>(call: () => Promise<T>, viewOf: (result: T) => McpView, done?: (result: T) => void): Promise<T> => {
    const my = ++seq.current;
    pendingRef.current += 1;
    setPending((n) => n + 1);
    return call()
      .then((result) => {
        if (my > applied.current) {
          applied.current = my;
          setView(viewOf(result));
          setLoadError(null);
        }
        done?.(result);
        return result;
      })
      .finally(() => {
        pendingRef.current -= 1;
        setPending((n) => n - 1);
      });
  };
  const act = (call: () => Promise<McpView>, done?: () => void): Promise<void> => {
    setActionError(null);
    return track(call, (v) => v, done).then(() => undefined);
  };
  const fail = (e: unknown) => setActionError(errorText(e));

  const onRemove = (name: string) => {
    act(
      () => api.mcpRemoveServer(name),
      () => {
        setAskRemove(null);
        setLogs((l) => (l?.name === name ? null : l));
        setPanel((p) => (p?.kind === 'edit' && p.name === name ? null : p));
      },
    ).catch(fail);
  };

  return (
    <McpSection
      view={view}
      loadError={loadError}
      actionError={actionError}
      busy={pending > 0}
      panel={panel}
      logs={logs}
      askRemove={askRemove}
      onToggleEnabled={(name, enabled) => void act(() => api.mcpSetEnabled(name, enabled)).catch(fail)}
      onServerApproval={(name, level) => void act(() => api.mcpSetServerApproval(name, level)).catch(fail)}
      onToolApproval={(name, tool, level) => void act(() => api.mcpSetToolApproval(name, tool, level)).catch(fail)}
      onRestart={(name) => void act(() => api.mcpRestart(name)).catch(fail)}
      onToggleLogs={(name) => {
        setLogs((l) => (l?.name === name ? null : { name, lines: null, error: null }));
      }}
      onAskRemove={setAskRemove}
      onRemove={onRemove}
      onEdit={(name) => {
        setActionError(null);
        setPanel({ kind: 'edit', name });
      }}
      onOpenAdd={() => {
        setActionError(null);
        setPanel({ kind: 'add' });
      }}
      onClosePanel={() => setPanel(null)}
      onSave={(input) => act(() => api.mcpSaveServer(input), () => setPanel(null))}
      onImport={(text) => track(() => api.mcpImport(text), (r) => r.view)}
    />
  );
}
