import { createRoot } from "react-dom/client";
import { App } from "./App";
import {
  applyDocumentTheme,
  getInitialThemeMode,
  resolveTheme,
} from "./theme";
import "@blueprintjs/core/lib/css/blueprint.css";
import "./styles.css";

const initialThemeMode = getInitialThemeMode();
applyDocumentTheme(resolveTheme(initialThemeMode));

createRoot(document.getElementById("root")!).render(
  <App initialThemeMode={initialThemeMode} />,
);
