// SPDX-License-Identifier: GPL-3.0-only

import { StrictMode, useEffect } from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter } from "react-router-dom";
import { Subscribe } from "@react-rxjs/core";
import App from "./App";
import { connect } from "@/state/chain.state";
import { getParachainWs } from "@/state/network.state";
import { restoreSigner } from "@/state/wallet.state";
import "./styles/index.css";

const basename = import.meta.env.BASE_URL.replace(/\/$/, "") || "/";

function AppWithInit() {
  useEffect(() => {
    connect(getParachainWs())
      .then(() => restoreSigner())
      .catch(() => { /* surfaced via useConnectionError */ });
  }, []);

  return <App />;
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <Subscribe fallback={<div className="min-h-screen bg-background" />}>
      <BrowserRouter basename={basename}>
        <AppWithInit />
      </BrowserRouter>
    </Subscribe>
  </StrictMode>,
);
