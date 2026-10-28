import { useEffect, useState } from "react";

export type Theme = "light" | "dark";

const STORAGE_KEY = "stagewhisper-theme";
const BG_COLORS: Record<Theme, string> = { light: "#f5f0ec", dark: "#0a0a0a" };

function getInitialTheme(): Theme {
  const stored = localStorage.getItem(STORAGE_KEY);
  if (stored === "light" || stored === "dark") return stored;
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

function applyTheme(theme: Theme) {
  document.documentElement.setAttribute("data-theme", theme);
  document.documentElement.style.background = BG_COLORS[theme];
}

export function initTheme() {
  const theme = getInitialTheme();
  applyTheme(theme);
  return theme;
}

export function useTheme() {
  const [theme, setThemeState] = useState<Theme>(getInitialTheme);

  const setTheme = (next: Theme) => {
    setThemeState(next);
    localStorage.setItem(STORAGE_KEY, next);
    applyTheme(next);
  };

  const toggle = () => setTheme(theme === "dark" ? "light" : "dark");

  useEffect(() => {
    applyTheme(theme);
  }, [theme]);

  return { theme, setTheme, toggle } as const;
}
