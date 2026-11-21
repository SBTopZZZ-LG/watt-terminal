// The native side of attention: the desktop notification, the taskbar flash (Windows) or dock bounce (macOS), the window's
// focus, and the click on a notification. The only module that imports the notification and window plugins, so the policy
// (attention.ts) and the tests never load them. Nothing here runs at import time.

import { isPermissionGranted, onAction, requestPermission, sendNotification } from '@tauri-apps/plugin-notification';
import { getCurrentWindow, UserAttentionType } from '@tauri-apps/api/window';
import type { AttentionDecision } from './attention';

/**
 * Shows the notification and asks for the user's attention. Each step has its own try: a refused permission or a failed
 * flash never reaches the UI. Desktop permission is granted without a prompt; the request is only made when it is not.
 */
export async function sendAttention(decision: AttentionDecision): Promise<void> {
  try {
    const granted = (await isPermissionGranted()) || (await requestPermission()) === 'granted';
    if (granted) {
      sendNotification({ title: decision.title, body: decision.body });
    }
  } catch {
    // No notification. The flash below still runs.
  }
  try {
    await getCurrentWindow().requestUserAttention(
      decision.attention === 'critical' ? UserAttentionType.Critical : UserAttentionType.Informational,
    );
  } catch {
    // No flash. Nothing else to do.
  }
}

/**
 * Reports the window's native focus now and on every change. Returns the unlisten function. The listener is registered
 * before the first read, so a change during that read is not missed.
 */
export async function watchWindowFocus(onChange: (focused: boolean) => void): Promise<() => void> {
  const win = getCurrentWindow();
  const unlisten = await win.onFocusChanged(({ payload }) => onChange(payload));
  try {
    onChange(await win.isFocused());
  } catch {
    // The caller keeps its default (focused), so nothing is sent while the state is unknown.
  }
  return unlisten;
}

/** Calls `onClick` when the user clicks a Watt notification. Returns the function that stops listening. */
export async function onNotificationClick(onClick: () => void): Promise<() => void> {
  const listener = await onAction(() => onClick());
  return () => {
    void listener.unregister().catch(() => undefined);
  };
}
