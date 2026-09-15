import { createRoot } from "react-dom/client";
import { App } from "./App";
import "@blueprintjs/core/lib/css/blueprint.css";
import "./styles.css";

createRoot(document.getElementById("root")!).render(<App />);
