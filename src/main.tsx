import React from "react";
import ReactDOM from "react-dom/client";
import "@fontsource/ibm-plex-mono/latin-400.css";
import "@fontsource/ibm-plex-mono/latin-500.css";
import "@fontsource/ibm-plex-mono/latin-600.css";
import App from "./App";
import { ImageViewerProvider } from "./components/ImageViewer";
import { installDropGuard } from "./lib/dropGuard";
import { installWebviewGuards } from "./lib/webviewGuards";

// A file dropped outside the agent input must not open in the webview. The agent input takes its own drops (see InputBar).
installDropGuard(document);
// The webview's own accelerators (reload, print, save...), middle-click autoscroll, and dragging an image or a link.
installWebviewGuards(document);

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ImageViewerProvider>
      <App />
    </ImageViewerProvider>
  </React.StrictMode>,
);
