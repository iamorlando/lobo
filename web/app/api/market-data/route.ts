import { marketDataResponse } from "@/lib/market-data.mjs";
export const runtime = "nodejs";
// Public exchanges do not all send CORS headers. This bounded transport proxy
// forwards approved bootstrap resources; decoding remains in the Rust adapter.
export async function GET(request: Request) {
  return marketDataResponse(request);
}
