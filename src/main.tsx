import React from "react";
import ReactDOM from "react-dom/client";
import "@fontsource/ibm-plex-mono/latin-400.css";
import "@fontsource/ibm-plex-mono/latin-500.css";
import "@fontsource/ibm-plex-mono/latin-600.css";
import App from "./App";
import { installDropGuard } from "./lib/dropGuard";

// A file dropped outside the agent input must not open in the webview. The agent input takes its own drops (see InputBar).
installDropGuard(document);

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
