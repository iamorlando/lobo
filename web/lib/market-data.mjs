const allowed = new Set([
  "https://api-pub.bitfinex.com/v2/conf/pub:list:pair:exchange",
]);

export async function marketDataResponse(request, fetcher = fetch) {
  const url = new URL(request.url).searchParams.get("url");
  if (!allowed.has(url))
    return new Response("Unsupported market data resource", { status: 400 });
  try {
    const response = await fetcher(url, {
      signal: AbortSignal.timeout(10000),
      redirect: "error",
    });
    if (!response.ok)
      return new Response("Instrument directory unavailable", { status: 502 });
    const bytes = new Uint8Array(await response.arrayBuffer());
    if (bytes.length > 1048576)
      return new Response("Instrument directory too large", { status: 502 });
    return new Response(bytes, {
      headers: {
        "Content-Type": "application/json",
        "Cache-Control": "public, max-age=300",
      },
    });
  } catch {
    return new Response("Cannot load the public instrument directory", {
      status: 502,
    });
  }
}
