/// <reference types="svelte" />
/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** Discord application (client) id; required to boot the SDK. */
  readonly VITE_DISCORD_CLIENT_ID: string;
  /** Optional API base override; defaults to same-origin `/api`. */
  readonly VITE_API_BASE?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
