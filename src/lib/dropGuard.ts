// A file dropped anywhere in the window must not open in the webview (the browser navigates to a dropped file unless the drop
// is cancelled). Drops on the agent input are accepted by InputBar. Every other drag of files is cancelled here. A drag of
// text or of a page element is not touched.

/** Whether a drag carries files from outside the page (the DataTransfer lists "Files" among its types). */
export function carriesFiles(dataTransfer: DataTransfer | null | undefined): boolean {
  return dataTransfer !== null && dataTransfer !== undefined && Array.from(dataTransfer.types).includes('Files');
}

/**
 * Cancels dragover and drop for drags that carry files, in the capture phase, so no element can let one through. Returns the
 * function that removes the listeners.
 */
export function installDropGuard(target: Pick<EventTarget, 'addEventListener' | 'removeEventListener'> = document): () => void {
  const cancel = (e: Event) => {
    if (carriesFiles((e as DragEvent).dataTransfer)) {
      e.preventDefault();
    }
  };
  target.addEventListener('dragover', cancel, true);
  target.addEventListener('drop', cancel, true);
  // The options form matches the listeners added above with capture; Node 22's EventTarget does not match a boolean capture here.
  return () => {
    target.removeEventListener('dragover', cancel, { capture: true });
    target.removeEventListener('drop', cancel, { capture: true });
  };
}
