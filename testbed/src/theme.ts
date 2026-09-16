import { useCallback, useEffect, useState } from "react";

export type ThemeMode = "light" | "dark" | "system";
export type ResolvedTheme = Exclude<ThemeMode, "system">;

export const THEME_STORAGE_KEY = "troute.testbed.theme";
const SYSTEM_DARK_QUERY = "(prefers-color-scheme: dark)";

function isThemeMode(value: string | null): value is ThemeMode {
  return value === "light" || value === "dark" || value === "system";
}

function systemPrefersDark() {
  return (
    typeof window !== "undefined" &&
    window.matchMedia?.(SYSTEM_DARK_QUERY).matches === true
  );
}

export function getInitialThemeMode(): ThemeMode {
  try {
    const stored = window.localStorage.getItem(THEME_STORAGE_KEY);
    return isThemeMode(stored) ? stored : "system";
  } catch {
    return "system";
  }
}

export function resolveTheme(mode: ThemeMode): ResolvedTheme {
  if (mode === "system") return systemPrefersDark() ? "dark" : "light";
  return mode;
}

export function saveThemeMode(mode: ThemeMode) {
  try {
    window.localStorage.setItem(THEME_STORAGE_KEY, mode);
  } catch {
    // Theme selection remains usable when browser storage is unavailable.
  }
}

export function applyDocumentTheme(theme: ResolvedTheme) {
  document.documentElement.dataset.theme = theme;
}

export function useTheme(initialMode = getInitialThemeMode()) {
  const [mode, setMode] = useState<ThemeMode>(initialMode);
  const [systemDark, setSystemDark] = useState(systemPrefersDark);
  const resolvedTheme: ResolvedTheme =
    mode === "system" ? (systemDark ? "dark" : "light") : mode;

  useEffect(() => {
    applyDocumentTheme(resolvedTheme);
  }, [resolvedTheme]);

  useEffect(() => {
    if (mode !== "system") return;

    const media = window.matchMedia(SYSTEM_DARK_QUERY);
    const handleChange = (event: MediaQueryListEvent) => {
      setSystemDark(event.matches);
    };

    setSystemDark(media.matches);
    media.addEventListener("change", handleChange);
    return () => media.removeEventListener("change", handleChange);
  }, [mode]);

  const selectMode = useCallback((nextMode: ThemeMode) => {
    if (nextMode === "system") setSystemDark(systemPrefersDark());
    saveThemeMode(nextMode);
    setMode(nextMode);
  }, []);

  return { mode, resolvedTheme, selectMode };
}
