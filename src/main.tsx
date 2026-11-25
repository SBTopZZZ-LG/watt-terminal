import React from "react";
import ReactDOM from "react-dom/client";
import "@fontsource/ibm-plex-mono/latin-400.css";
import "@fontsource/ibm-plex-mono/latin-500.css";
import "@fontsource/ibm-plex-mono/latin-600.css";
import App from "./App";
import { ErrorBoundary, installCrashLogging } from "./components/ErrorBoundary";
import { ImageViewerProvider } from "./components/ImageViewer";
import { installDropGuard } from "./lib/dropGuard";
import { installWebviewGuards } from "./lib/webviewGuards";

// A file dropped outside the agent input must not open in the webview. The agent input takes its own drops (see InputBar).
installDropGuard(document);
// The webview's own accelerators (reload, print, save...), middle-click autoscroll, and dragging an image or a link.
installWebviewGuards(document);
// Uncaught errors and unhandled rejections go to the console (they do not unmount React, so the boundary below does not see them).
installCrashLogging(window);

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ImageViewerProvider>
      {/* A render error below here shows the fallback in ErrorBoundary.tsx instead of a blank window. */}
      <ErrorBoundary>
        <App />
      </ErrorBoundary>
    </ImageViewerProvider>
  </React.StrictMode>,
);
