import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import SettingsStore from "./settings";
import { createAppMenu, fetchThemeMenuItem } from "./appMenu";
import { setTheme } from "@tauri-apps/api/app";
// import { getCurrentWindow } from "@tauri-apps/api/window";

const settingsStore = await SettingsStore.create();

const initialTheme = (await settingsStore.getSettings()).theme ?? "system";

const menu = await createAppMenu(async (theme) => {
    console.log(`Theme selected: ${theme}`);
    await settingsStore.setTheme(theme);

    // TODO: respond to the actual event.
    for (const t of ['system', 'light', 'dark'] as const) {
        const item = await fetchThemeMenuItem(menu, t);
        item.setChecked(t === theme);
    }

    const appTheme = (theme === "system") ? null : theme;
    await setTheme(appTheme);
}, initialTheme);
// The native theme resets each launch, so reapply the saved choice.
await setTheme(initialTheme === "system" ? null : initialTheme);
const previous = await menu.setAsAppMenu();
previous?.close();

// TODO: event doesn't fire.
//
// const window = getCurrentWindow();
// window.onThemeChanged((e) => {
//     const theme = e.payload;
//     console.log(`Theme changed: ${theme}`);
//     for (const t of ['system', 'light', 'dark'] as const) {
//         const isTheme = theme === null ? t === 'system' : t === theme;
//         fetchThemeMenuItem(menu, t).then(item => {
//             item.setChecked(isTheme);
//         });
//     }
// });

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App settings={settingsStore} />
  </React.StrictMode>,
);
