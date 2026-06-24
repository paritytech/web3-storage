// SPDX-License-Identifier: GPL-3.0-only

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import "./app.css";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
