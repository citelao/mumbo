import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import SettingsStore from "./settings";

const settingsStore = await SettingsStore.create();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App settings={settingsStore} />
  </React.StrictMode>,
);
