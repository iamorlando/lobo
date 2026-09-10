"use client";
import { useEffect, useMemo, useState } from "react";
import { matchTickers } from "@/lib/ticker-match";

export function TickerPicker({
  value,
  tickers,
  venue,
  onSelect,
  disabled = false,
}: {
  value: string;
  tickers: string[];
  venue: string;
  onSelect(ticker: string): void;
  disabled?: boolean;
}) {
  const [draft, setDraft] = useState(value);
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  useEffect(() => {
    setDraft(value);
  }, [value]);
  const [message, setMessage] = useState("");
  const query = draft.trim().toUpperCase();
  const matches = useMemo(
    () => matchTickers(tickers, query, query === value),
    [tickers, query, value],
  );
  useEffect(() => {
    if (open)
      document
        .getElementById(`ticker-option-${active}`)
        ?.scrollIntoView({ block: "nearest" });
  }, [active, open]);
  const suggestions = matches.slice(0, 30);
  function select(symbol: string) {
    if (disabled) return;
    if (!tickers.includes(symbol)) {
      setMessage(
        tickers.length
          ? "Choose a symbol listed in this feed."
          : "Reading the ticker directory…",
      );
      return;
    }
    onSelect(symbol);
    setDraft(symbol);
    setMessage("");
    setOpen(false);
  }
  return (
    <form
      className="symbol-form"
      onSubmit={(event) => {
        event.preventDefault();
        select(query);
      }}
    >
      <label htmlFor="ticker">SYMBOL</label>
      <div className="ticker-control">
        <input
          id="ticker"
          disabled={disabled}
          role="combobox"
          aria-autocomplete="list"
          aria-expanded={open}
          aria-controls={open ? "ticker-options" : undefined}
          aria-activedescendant={
            open && suggestions[active] ? `ticker-option-${active}` : undefined
          }
          aria-describedby="ticker-help"
          autoComplete="off"
          value={draft}
          maxLength={64}
          spellCheck={false}
          onFocus={(event) => {
            setOpen(true);
            setActive(0);
            event.target.select();
          }}
          onBlur={() => setOpen(false)}
          onChange={(event) => {
            setDraft(event.target.value.toUpperCase());
            setOpen(true);
            setActive(0);
            setMessage("");
          }}
          onKeyDown={(event) => {
            if (event.key === "ArrowDown" || event.key === "ArrowUp") {
              event.preventDefault();
              setOpen(true);
              setActive((index) =>
                Math.max(
                  0,
                  Math.min(
                    suggestions.length - 1,
                    index + (event.key === "ArrowDown" ? 1 : -1),
                  ),
                ),
              );
            } else if (event.key === "Escape") {
              setOpen(false);
              setDraft(value);
              setMessage("");
            } else if (event.key === "Enter" && open && suggestions[active]) {
              event.preventDefault();
              select(suggestions[active]);
            }
          }}
        />
        <button type="submit" aria-label="Load symbol">
          ↵
        </button>
        {open && (
          <ul
            id="ticker-options"
            role="listbox"
            aria-label="Feed tickers"
            className="ticker-options"
          >
            {suggestions.map((symbol, index) => (
              <li
                key={symbol}
                id={`ticker-option-${index}`}
                role="option"
                aria-selected={index === active}
                onMouseDown={(event) => event.preventDefault()}
                onMouseMove={() => setActive(index)}
                onClick={() => select(symbol)}
              >
                <strong>{symbol}</strong>
                {symbol === value && <span>Viewing</span>}
              </li>
            ))}
            {!suggestions.length && (
              <li role="presentation" className="ticker-empty">
                {tickers.length ? "No matching tickers" : "Reading directory…"}
              </li>
            )}
            {matches.length > 30 && (
              <li role="presentation" className="ticker-empty">
                Type to filter {matches.length.toLocaleString()} symbols
              </li>
            )}
          </ul>
        )}
      </div>
      <span id="ticker-help" aria-live="polite">
        {message || `${tickers.length.toLocaleString()} symbols · ${venue}`}
      </span>
    </form>
  );
}
