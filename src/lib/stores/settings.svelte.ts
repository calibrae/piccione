import { invoke } from "@tauri-apps/api/core";

export type Theme = "light" | "dark" | "auto";

export interface Settings {
  read_receipts: boolean;
  typing_indicators: boolean;
  theme: Theme;
}

const DEFAULTS: Settings = {
  read_receipts: true,
  typing_indicators: true,
  theme: "auto",
};

function createSettingsStore() {
  // Optimistic in-memory copy. Initialised to defaults so the UI renders
  // before the backend has had a chance to send the real values.
  let current = $state<Settings>({ ...DEFAULTS });
  let loaded = $state(false);

  /**
   * Apply the theme to the document root by toggling a class. The CSS
   * declares all colours as variables under `:root`, `[data-theme="dark"]`,
   * and `[data-theme="light"]` so changing the attribute swaps the palette
   * without re-rendering anything.
   */
  function applyTheme(t: Theme) {
    const effective =
      t === "auto"
        ? window.matchMedia("(prefers-color-scheme: dark)").matches
          ? "dark"
          : "light"
        : t;
    document.documentElement.setAttribute("data-theme", effective);
  }

  // Watch for OS theme changes when we're on "auto".
  if (typeof window !== "undefined" && window.matchMedia) {
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    mq.addEventListener("change", () => {
      if (current.theme === "auto") applyTheme("auto");
    });
  }

  return {
    get current() {
      return current;
    },
    get loaded() {
      return loaded;
    },

    async load() {
      try {
        const s = await invoke<Settings>("get_settings");
        current = s;
        applyTheme(s.theme);
        loaded = true;
      } catch (e) {
        console.warn("get_settings failed, using defaults:", e);
        applyTheme(current.theme);
        loaded = true;
      }
    },

    async update(partial: Partial<Settings>) {
      const next = { ...current, ...partial };
      try {
        await invoke("set_settings", { settings: next });
        current = next;
        if (partial.theme) applyTheme(next.theme);
      } catch (e) {
        console.error("set_settings failed:", e);
        throw e;
      }
    },

    async signOut(): Promise<{
      keychain_cleared: boolean;
      db_key_file_removed: boolean;
      db_key_bak_removed: boolean;
    }> {
      return await invoke("sign_out");
    },
  };
}

export const settingsStore = createSettingsStore();
