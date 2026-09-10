import { open, stat } from "node:fs/promises";
import path from "node:path";
export const runtime = "nodejs";
// Only this configured local file is accessible. Requests cannot choose paths.
const replayPath = () =>
  path.resolve(
    /* turbopackIgnore: true */ process.env.LOBO_ITCH_PATH ??
      path.join(process.cwd(), "../data/NASDAQ/01302020.NASDAQ_ITCH50"),
  );
export async function GET(request: Request) {
  const url = new URL(request.url);
  try {
    const filePath = replayPath();
    const info = await stat(/* turbopackIgnore: true */ filePath);
    if (!info.isFile())
      return new Response("Replay source is not a file", { status: 400 });
    if (url.searchParams.has("info"))
      return Response.json({ name: path.basename(filePath), size: info.size });
    const offset = Number(url.searchParams.get("offset") ?? 0),
      length = Number(url.searchParams.get("length") ?? 1048576);
    if (
      !Number.isSafeInteger(offset) ||
      offset < 0 ||
      !Number.isSafeInteger(length) ||
      length < 1 ||
      length > 1048576
    )
      return new Response("Invalid byte range", { status: 400 });
    const file = await open(/* turbopackIgnore: true */ filePath, "r");
    try {
      const buffer = Buffer.alloc(
        Math.min(length, Math.max(0, info.size - offset)),
      );
      const { bytesRead } = await file.read(buffer, 0, buffer.length, offset);
      return new Response(new Uint8Array(buffer.subarray(0, bytesRead)), {
        headers: {
          "Content-Type": "application/octet-stream",
          "Cache-Control": "no-store",
        },
      });
    } finally {
      await file.close();
    }
  } catch {
    return new Response(
      "Repository ITCH file unavailable. Select a local file or set LOBO_ITCH_PATH.",
      { status: 404 },
    );
  }
}
