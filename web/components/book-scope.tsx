"use client";
import { useMemo, useRef, useState } from "react";
import { matchTickers } from "@/lib/ticker-match";
import categories from "@/lib/ticker-categories.json";
export type BookScope = string[] | null;
const companies: Record<string, string> = categories.holdings;

export function BookScopeSelector({
  value,
  tickers,
  live,
  onApply,
}: {
  value: BookScope;
  tickers: string[];
  live: boolean;
  onApply(scope: BookScope): void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [draft, setDraft] = useState<BookScope>(value);
  const [query, setQuery] = useState("");
  const [limit, setLimit] = useState(120);
  // Replay directories arrive incrementally. Keep the requested preset for the
  // adapter, but show only its symbols that actually exist in this session.
  const availableValue = useMemo(
    () =>
      value?.filter((symbol) => !tickers.length || tickers.includes(symbol)) ??
      null,
    [value, tickers],
  );
  const selected = useMemo(() => new Set(draft ?? tickers), [draft, tickers]);
  const matches = useMemo(() => matchTickers(tickers, query), [tickers, query]);
  const tech = tickers.filter((symbol) => categories.tech.includes(symbol));
  const sp500 = tickers.filter((symbol) => symbol in companies);
  const toggle = (symbol: string) => {
    const next = new Set(selected);
    if (next.has(symbol)) next.delete(symbol);
    else next.add(symbol);
    setDraft([...next].sort());
  };
  const preset = (symbols: string[]) =>
    setDraft([...new Set([...(draft ?? []), ...symbols])].sort());
  const close = () => dialog.current?.close();
  return (
    <>
      <button
        className="scope-trigger outline"
        onClick={() => {
          setDraft(availableValue);
          setQuery("");
          setLimit(120);
          dialog.current?.showModal();
        }}
        aria-haspopup="dialog"
      >
        <span className="field-label">BOOK SCOPE</span>
        <strong>
          {availableValue === null
            ? "All tickers"
            : `${availableValue.length.toLocaleString()} ${availableValue.length === 1 ? "ticker" : "tickers"}`}
        </strong>
        <span>⌄</span>
      </button>
      <dialog
        ref={dialog}
        className="scope-dialog"
        aria-labelledby="scope-title"
        onClick={(event) => {
          if (event.target === event.currentTarget) close();
        }}
      >
        <div className="scope-content">
          <header>
            <div>
              <span className="field-label">
                {live ? "LIVE" : "REPLAY"} · BOOK SCOPE
              </span>
              <h2 id="scope-title">Select the books to load</h2>
              <p>
                {live
                  ? "All follows pairs as you visit them. A selection keeps every chosen pair live."
                  : "Only scoped tickers build books. Applying a new scope restarts at the configured session start."}
              </p>
            </div>
            <button onClick={close} aria-label="Close book scope">
              ×
            </button>
          </header>
          <div className="scope-search">
            <span aria-hidden="true">⌕</span>
            <input
              autoFocus
              aria-label="Search book scope tickers"
              placeholder="Search the feed's ticker directory…"
              value={query}
              onChange={(event) => {
                setQuery(event.target.value.toUpperCase());
                setLimit(120);
              }}
              onKeyDown={(event) => {
                if (event.key === "ArrowDown") {
                  event.preventDefault();
                  dialog.current
                    ?.querySelector<HTMLInputElement>(".scope-ticker input")
                    ?.focus();
                }
                if (event.key === "Enter" && matches[0]) {
                  event.preventDefault();
                  toggle(matches[0]);
                }
              }}
              autoComplete="off"
              spellCheck={false}
            />
            <span>{matches.length.toLocaleString()} tickers</span>
          </div>
          <div className="scope-presets">
            <button
              className={draft === null ? "selected" : "outline"}
              onClick={() => setDraft(null)}
            >
              All tickers
            </button>
            {!!tech.length && (
              <button onClick={() => preset(tech)}>
                + Top tech · {tech.length}
              </button>
            )}
            {!!sp500.length && (
              <button onClick={() => preset(sp500)}>
                + S&P 500 · {sp500.length}
              </button>
            )}
            <button className="subtle" onClick={() => setDraft([])}>
              Clear all
            </button>
          </div>
          <section className="scope-selection" aria-label="Selected scope">
            <span className="field-label">
              {draft === null
                ? "ALL FEED TICKERS"
                : `SELECTED · ${draft.length.toLocaleString()}`}
            </span>
            <div className="scope-chips">
              {draft === null ? (
                <span>All current and newly discovered tickers</span>
              ) : !draft.length ? (
                <span>Select tickers below, or choose a category.</span>
              ) : (
                <>
                  {draft.slice(0, 24).map((symbol) => (
                    <button
                      key={symbol}
                      onClick={() => toggle(symbol)}
                      aria-label={`Remove ${symbol}`}
                    >
                      {symbol}
                      <span>×</span>
                    </button>
                  ))}
                  {draft.length > 24 && <span>+{draft.length - 24} more</span>}
                </>
              )}
            </div>
          </section>
          <div className="scope-results" aria-label="Available feed tickers">
            <div className="scope-grid">
              {matches.slice(0, limit).map((symbol) => (
                <label
                  key={symbol}
                  className={
                    selected.has(symbol)
                      ? "scope-ticker checked"
                      : "scope-ticker"
                  }
                >
                  <input
                    type="checkbox"
                    checked={selected.has(symbol)}
                    onChange={() => toggle(symbol)}
                    onKeyDown={(event) => {
                      if (event.key === "Enter") {
                        event.preventDefault();
                        toggle(symbol);
                      }
                    }}
                  />
                  <span>
                    <strong>{symbol}</strong>
                    <small title={companies[symbol]}>
                      {companies[symbol] || "Feed instrument"}
                    </small>
                  </span>
                  <span className="scope-check" aria-hidden="true">
                    {selected.has(symbol) ? "✓" : "+"}
                  </span>
                </label>
              ))}
            </div>
            {!matches.length && (
              <p className="scope-empty">
                {tickers.length
                  ? "No matching tickers in this feed."
                  : "The ticker directory will appear as the source loads."}
              </p>
            )}
            {matches.length > limit && (
              <button
                className="outline scope-more"
                onClick={() => setLimit((n) => n + 120)}
              >
                Show more · {matches.length - limit} remaining
              </button>
            )}
          </div>
          <footer>
            <div>
              <strong>
                {draft === null
                  ? "All tickers"
                  : `${draft.length.toLocaleString()} tickers selected`}
              </strong>
              {!!sp500.length && (
                <small>
                  Categories intersect this feed.{" "}
                  <a href={categories.source} target="_blank" rel="noreferrer">
                    SPY holdings · {categories.asOf}
                  </a>
                  . Top tech is a short curated list.
                </small>
              )}
            </div>
            <button className="subtle" onClick={close}>
              Cancel
            </button>
            <button
              className="primary"
              disabled={draft?.length === 0}
              onClick={() => {
                onApply(draft);
                close();
              }}
            >
              Apply scope
            </button>
          </footer>
        </div>
      </dialog>
    </>
  );
}
