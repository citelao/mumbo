import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import SettingsStore from "./settings";
import { createAppMenu } from "./appMenu";

const settingsStore = await SettingsStore.create();

const menu = await createAppMenu();
await menu.setAsAppMenu();

// TEST: Update individual menu item text
const statusItem = await menu.get('status');
if (statusItem) {
  await statusItem.setText('Status: Ready');
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App settings={settingsStore} />
  </React.StrictMode>,
);
