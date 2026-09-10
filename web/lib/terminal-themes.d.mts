export interface TerminalTheme {
  background: string;
  foreground: string;
  selection: string;
  ansi: string[];
  brights: string[];
}
export const themeRepository: string;
export const themeCatalogUrl: string;
export function themeUrl(name: string): string;
export function parseTheme(source: string): TerminalTheme;
export function mix(a: string, b: string, weight: number): string;
export function luminance(hex: string): number;
export function themeTokens(theme: TerminalTheme): {
  mode: "light" | "dark";
  tokens: Record<string, string>;
};
