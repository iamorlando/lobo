/** Shared ranking for the viewing ticker and book-scope autocomplete. */
export function matchTickers(
  tickers: string[],
  query: string,
  includeAll = false,
) {
  const normalized = query.trim().toUpperCase().replace(/\s+/g, "-");
  return tickers
    .filter((symbol) => includeAll || symbol.includes(normalized))
    .sort(
      (a, b) =>
        Number(b.startsWith(normalized)) - Number(a.startsWith(normalized)) ||
        a.localeCompare(b),
    );
}
