import test from "node:test";
import assert from "node:assert/strict";
import { marketDataResponse } from "../lib/market-data.mjs";
const directory = "https://api-pub.bitfinex.com/v2/conf/pub:list:pair:exchange";
const request = (url) =>
  new Request(
    `http://localhost/api/market-data?url=${encodeURIComponent(url)}`,
  );
test("directory transport preserves opaque bytes and rejects unapproved resources", async () => {
  let calls = 0;
  const fetcher = async (url, options) => {
    calls++;
    assert.equal(url, directory);
    assert.equal(options.redirect, "error");
    return new Response('[["BTCUSD","AAVE:USD"]]');
  };
  for (const url of [
    "http://127.0.0.1/",
    "file:///etc/passwd",
    `${directory}?other=1`,
    "https://api-pub.bitfinex.com/v2/auth/w/order/submit",
  ]) {
    assert.equal((await marketDataResponse(request(url), fetcher)).status, 400);
  }
  assert.equal(calls, 0);
  const response = await marketDataResponse(request(directory), fetcher);
  assert.equal(response.status, 200);
  assert.equal(await response.text(), '[["BTCUSD","AAVE:USD"]]');
  assert.equal(calls, 1);
});
test("directory transport reports upstream failure and bounds response size", async () => {
  for (const fetcher of [
    async () => new Response("no", { status: 503 }),
    async () => {
      throw new Error("timeout");
    },
    async () => new Response(new Uint8Array(1048577)),
  ]) {
    assert.equal(
      (await marketDataResponse(request(directory), fetcher)).status,
      502,
    );
  }
});
