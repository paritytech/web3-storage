// SPDX-License-Identifier: GPL-3.0-only

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { Subscribe } from "@react-rxjs/core";
import { BrowserRouter } from "react-router-dom";
import App from "./App";
import { connect } from "@/state/chain.state";
import { getParachainWs } from "@/state/network.state";
import { restoreSigner } from "@/state/wallet.state";
import "./app.css";

const basename = import.meta.env.BASE_URL.replace(/\/$/, "") || "/";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <Subscribe fallback={<div className="min-h-screen bg-background" />}>
      <BrowserRouter basename={basename}>
        <App />
      </BrowserRouter>
    </Subscribe>
  </StrictMode>,
);

// Boot: connect to the parachain WS and restore any persisted signer.
connect(getParachainWs());
restoreSigner();
