import { createContext, useCallback, useContext, useEffect, useRef, useState } from 'react';
import type { KeyboardEvent, MouseEvent, ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { api, CHAT_ID } from '../lib/api';
import { fitImage } from '../lib/imageViewer';
import type { Size } from '../lib/imageViewer';

/**
 * An image to view. `full` is a data URL already in hand (a draft in the composer). Otherwise `sha256` and `mime` ask the
 * backend for the stored full image of the open conversation. `thumb` is shown meanwhile, or when the full image is unavailable.
 */
export interface ViewerImage {
  name: string;
  thumb?: string;
  full?: string;
  sha256?: string;
  mime?: string;
}

type OpenImage = (image: ViewerImage) => void;

/** Outside a provider (a static render) opening does nothing. */
const ImageViewerContext = createContext<OpenImage>(() => undefined);

/** The function that opens the image viewer. Only one image is open at a time. */
export function useImageViewer(): OpenImage {
  return useContext(ImageViewerContext);
}

function viewportSize(): Size {
  // No window in a static render: the size is read on mount.
  return typeof window === 'undefined' ? { w: 0, h: 0 } : { w: window.innerWidth, h: window.innerHeight };
}

/**
 * Provides the viewer to the app and draws it once, over everything. Each open is a new viewer (a token as its key), so an
 * image fetch from an earlier open never lands in a later one.
 */
export function ImageViewerProvider({ children }: { children: ReactNode }) {
  const [viewing, setViewing] = useState<{ image: ViewerImage; token: number } | null>(null);
  const tokens = useRef(0);
  const open = useCallback<OpenImage>((image) => {
    tokens.current += 1;
    setViewing({ image, token: tokens.current });
  }, []);
  const close = useCallback(() => setViewing(null), []);
  return (
    <ImageViewerContext.Provider value={open}>
      {children}
      {viewing !== null && <ImageViewer key={viewing.token} image={viewing.image} onClose={close} />}
    </ImageViewerContext.Provider>
  );
}

/**
 * A dialog showing one image at its fitted size, with a caption and a close button. It has no zoom and no pan. Focus moves to
 * the close button on open and comes back to the opener on close, and Tab stays inside. Escape and a click on the backdrop
 * (not the image) close it; Escape does not reach the composer or any other handler.
 */
export function ImageViewer({ image, onClose, host }: { image: ViewerImage; onClose(): void; host?: HTMLElement | null }) {
  const closeRef = useRef(onClose);
  const dialogRef = useRef<HTMLDivElement>(null);
  const closeButton = useRef<HTMLButtonElement>(null);
  const backdropPressed = useRef(false);
  const [full, setFull] = useState<string | null>(image.full ?? null);
  const [unavailable, setUnavailable] = useState(false);
  const [broken, setBroken] = useState(false);
  const [natural, setNatural] = useState<Size | null>(null);
  const [viewport, setViewport] = useState<Size>(viewportSize);

  useEffect(() => {
    closeRef.current = onClose;
  });

  // Moves focus in, and gives it back to the opener when the viewer goes. Focus that leaves the dialog comes back to it.
  useEffect(() => {
    const dialog = dialogRef.current;
    if (dialog === null) {
      return;
    }
    const opener = document.activeElement;
    closeButton.current?.focus();
    const pull = (e: FocusEvent) => {
      if (e.target instanceof Node && !dialog.contains(e.target)) {
        closeButton.current?.focus();
      }
    };
    document.addEventListener('focusin', pull);
    return () => {
      document.removeEventListener('focusin', pull);
      if (opener instanceof HTMLElement && opener.isConnected && opener !== document.body) {
        opener.focus();
      }
    };
    // Runs once per mount: each open is a new mount (see ImageViewerProvider).
  }, []);

  // The stored full image, when the open is for a sent image. A result that arrives after the viewer is gone is dropped.
  const { sha256, mime, full: inHand } = image;
  useEffect(() => {
    if (inHand !== undefined || sha256 === undefined || mime === undefined) {
      return;
    }
    let live = true;
    // TODO(multi-chat D2): the chat the image's conversation belongs to. The default chat until then.
    api.getAttachment(CHAT_ID, sha256, mime).then(
      (url) => {
        if (live) {
          setFull(url);
        }
      },
      () => {
        if (live) {
          setUnavailable(true);
        }
      },
    );
    return () => {
      live = false;
    };
  }, [inHand, sha256, mime]);

  useEffect(() => {
    const onResize = () => setViewport(viewportSize());
    window.addEventListener('resize', onResize);
    return () => window.removeEventListener('resize', onResize);
  }, []);

  const shown = full ?? image.thumb ?? null;
  const loading = full === null && !unavailable && sha256 !== undefined && mime !== undefined;
  const size = fitImage(natural ?? { w: 0, h: 0 }, viewport);

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      closeRef.current();
      return;
    }
    if (e.key === 'Tab') {
      // The close button is the only stop, so Tab stays on it.
      e.preventDefault();
      e.stopPropagation();
      closeButton.current?.focus();
    }
  };

  const node = (
    <div
      className="image-viewer-backdrop"
      onMouseDown={(e: MouseEvent<HTMLDivElement>) => {
        backdropPressed.current = e.target === e.currentTarget;
      }}
      onClick={(e: MouseEvent<HTMLDivElement>) => {
        // Only a press and release on the backdrop itself closes it.
        if (backdropPressed.current && e.target === e.currentTarget) {
          closeRef.current();
        }
      }}
    >
      <div ref={dialogRef} className="image-viewer" role="dialog" aria-modal="true" aria-label={`Image: ${image.name}`} tabIndex={-1} onKeyDown={onKeyDown}>
        <button ref={closeButton} type="button" className="image-viewer-close" aria-label="Close image" onClick={() => closeRef.current()}>
          ✕
        </button>
        <div className="image-viewer-stage">
          {shown !== null && !broken && (
            <img
              className={loading ? 'image-viewer-img is-loading' : 'image-viewer-img'}
              src={shown}
              alt={image.name}
              draggable={false}
              style={{ width: `${size.w}px`, height: `${size.h}px` }}
              onLoad={(e) => setNatural({ w: e.currentTarget.naturalWidth, h: e.currentTarget.naturalHeight })}
              onError={() => setBroken(true)}
            />
          )}
          {loading && (
            <div className="image-viewer-status" role="status">
              Loading…
            </div>
          )}
          {unavailable && <div className="image-viewer-note">Full image unavailable.</div>}
          {broken && <div className="image-viewer-note">This image can&apos;t be shown.</div>}
        </div>
        <div className="image-viewer-caption">
          {image.name}
        </div>
      </div>
    </div>
  );
  // Drawn in the body by default. A null host (a static render, which has no body) draws it where it is.
  const target = host === undefined ? document.body : host;
  return target === null ? node : createPortal(node, target);
}
