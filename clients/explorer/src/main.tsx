import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App.js";
import "./styles.css";

declare const __DEFAULT_RPC_URL__: string;

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App rpcUrl={__DEFAULT_RPC_URL__} />
  </React.StrictMode>,
);
