import { createRoot } from "react-dom/client";
import "./styles.css";
import { initialTheme } from "./api";
import { Approval } from "./ApprovalView";

initialTheme();
createRoot(document.getElementById("root")!).render(<Approval />);
