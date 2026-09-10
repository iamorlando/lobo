/** Standalone Next deployment. The Rust host supplies its own configuration. */
export function GET() {
  return Response.json({ mode: "standalone" });
}
