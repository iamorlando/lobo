import type { Metadata } from "next";
import "./globals.css";
import { ThemeProvider } from "@/components/theme-provider";

// Follow the OS before first paint; the provider listens for later changes.
const themeScript = `document.documentElement.dataset.theme = matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark';`;
export const metadata: Metadata = {
  title: "lobo · Replay Lab",
  description: "Order book replay, rendered directly on the GPU.",
};
export default function Layout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en" suppressHydrationWarning>
      <head>
        <script dangerouslySetInnerHTML={{ __html: themeScript }} />
      </head>
      <body>
        <ThemeProvider>{children}</ThemeProvider>
      </body>
    </html>
  );
}
