"use client";
import { createContext, useContext, useEffect, useState } from "react";
import {
  parseTheme,
  themeCatalogUrl,
  themeTokens,
  themeUrl,
} from "@/lib/terminal-themes.mjs";

const defaultTheme = "Acid Lime";
const ThemeContext = createContext({
  selected: defaultTheme,
  applied: "System",
  names: [] as string[],
  pending: false,
  error: "",
  select: (_name: string) => {},
  retry: () => {},
});
export function ThemeProvider({ children }: { children: React.ReactNode }) {
  const [selected, setSelected] = useState(defaultTheme);
  const [applied, setApplied] = useState("System");
  const [names, setNames] = useState<string[]>([]);
  const [systemLight, setSystemLight] = useState(false);
  const [ready, setReady] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  const [catalogError, setCatalogError] = useState("");
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    const system = matchMedia("(prefers-color-scheme: light)");
    const changed = () => setSystemLight(system.matches);
    changed();
    try {
      setSelected(
        localStorage.getItem("lobo-terminal-theme") || defaultTheme,
      );
    } catch {}
    setReady(true);
    system.addEventListener("change", changed);
    return () => system.removeEventListener("change", changed);
  }, []);
  useEffect(() => {
    const abort = new AbortController();
    setCatalogError("");
    void fetch(themeCatalogUrl, { signal: abort.signal, cache: "force-cache" })
      .then(async (response) => {
        if (!response.ok)
          throw Error(
            "The upstream theme catalog is unavailable. Retry shortly.",
          );
        const entries: { name: string; type: string }[] = await response.json();
        setNames(
          entries
            .filter(
              (entry) => entry.type === "file" && entry.name.endsWith(".toml"),
            )
            .map((entry) => entry.name.slice(0, -5))
            .sort((a, b) => a.localeCompare(b)),
        );
      })
      .catch((cause) => {
        if (!abort.signal.aborted)
          setCatalogError(String(cause.message || cause));
      });
    return () => abort.abort();
  }, [attempt]);
  useEffect(() => {
    if (!ready) return;
    const abort = new AbortController();
    const name =
      selected === "System"
        ? systemLight
          ? "GitHub Light Default"
          : "Espresso"
        : selected;
    setPending(true);
    setError("");
    void fetch(themeUrl(name), { signal: abort.signal, cache: "force-cache" })
      .then(async (response) => {
        if (!response.ok)
          throw Error(`Could not load ${name} from the theme repository.`);
        const palette = themeTokens(parseTheme(await response.text()));
        if (abort.signal.aborted) return;
        const root = document.documentElement;
        for (const [token, color] of Object.entries(palette.tokens))
          root.style.setProperty(token, color);
        root.style.setProperty(
          "--overlay-shadow",
          `0 16px 64px ${palette.tokens["--bg"]}cc`,
        );
        root.dataset.theme = palette.mode;
        root.dataset.terminalTheme = name;
        setApplied(name);
      })
      .catch((cause) => {
        if (!abort.signal.aborted) setError(String(cause.message || cause));
      })
      .finally(() => {
        if (!abort.signal.aborted) setPending(false);
      });
    return () => abort.abort();
  }, [selected, systemLight, ready, attempt]);
  const select = (name: string) => {
    setSelected(name);
    try {
      localStorage.setItem("lobo-terminal-theme", name);
    } catch {}
  };
  return (
    <ThemeContext.Provider
      value={{
        selected,
        applied,
        names,
        pending,
        error: error || catalogError,
        select,
        retry: () => setAttempt((n) => n + 1),
      }}
    >
      {children}
    </ThemeContext.Provider>
  );
}
export const useTheme = () => useContext(ThemeContext).applied;
export function ThemeSelector() {
  const theme = useContext(ThemeContext);
  return (
    <div className="theme-control">
      <label>
        <span className="field-label">
          ITERM THEME {theme.pending ? "· LOADING" : ""}
        </span>
        <select
          aria-label="iTerm theme"
          value={theme.selected}
          onChange={(event) => theme.select(event.target.value)}
        >
          <option value="System">
            System · {theme.selected === "System" ? theme.applied : "Automatic"}
          </option>
          {!theme.names.includes(theme.selected) &&
            theme.selected !== "System" && <option>{theme.selected}</option>}
          {theme.names.map((name) => (
            <option key={name}>{name}</option>
          ))}
        </select>
      </label>
      {theme.error && (
        <button
          className="theme-error"
          title={theme.error}
          onClick={theme.retry}
        >
          Theme unavailable · Retry
        </button>
      )}
    </div>
  );
}
