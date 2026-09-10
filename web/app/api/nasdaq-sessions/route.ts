import { createNasdaqHandler } from "@/lib/nasdaq-sessions.mjs";
export const runtime = "nodejs";
export const GET = createNasdaqHandler();
