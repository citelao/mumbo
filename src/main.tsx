import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import SettingsStore, { Settings } from "./settings";

const settingsStore = await SettingsStore.create();
const settings = await settingsStore.getSettings();
const onSaveSettings = async (newSettings: Settings) => {
    // TODO: storing does not refresh cached state throughout app.
    await settingsStore.saveSettings(newSettings);
};

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App settings={settings} onSaveSettings={onSaveSettings} />
  </React.StrictMode>,
);
