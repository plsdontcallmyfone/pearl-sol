import { createRoot } from "react-dom/client";
import "./styles.css";
import { initialTheme } from "./api";
import { App } from "./App";

initialTheme();
createRoot(document.getElementById("root")!).render(<App />);
