/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** Set by the Tauri CLI when it runs the frontend build; unset for plain `vite`. */
  readonly TAURI_ENV_PLATFORM?: string;
}
