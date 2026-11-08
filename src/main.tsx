import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import SettingsStore from "./settings";
import { createAppMenu } from "./appMenu";
import { setTheme } from "@tauri-apps/api/app";
import { getCurrentWindow } from "@tauri-apps/api/window";

// The window starts hidden; always show it, even if startup fails, so an error
// doesn't leave the app invisible.
try {
    const settingsStore = await SettingsStore.create();

    const initialTheme = (await settingsStore.getSettings()).theme ?? "system";
    // The native theme resets each launch, so reapply the saved choice. Do this
    // first, while the window is still hidden (see `visible: false` in
    // tauri.conf.json), so the first paint is never in the wrong theme.
    await setTheme(initialTheme === "system" ? null : initialTheme);

    const menu = await createAppMenu(settingsStore, async (theme) => {
        console.log(`Theme selected: ${theme}`);
        await setTheme(theme === "system" ? null : theme);
    });
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
} finally {
    await getCurrentWindow().show();
}
