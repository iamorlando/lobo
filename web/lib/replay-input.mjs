const CHUNK_BYTES = 1048576;
export const REMOTE_PREFETCH_RANGES = 8;
// Keep remote reads ahead of reconstruction without downloading a whole session.
// Ranges may arrive out of order; the decoder must receive them in file order.
// The compressed file size is transport progress, never the decoded size.
export function replayInput({
  size,
  compressed,
  readAt,
  signal,
  prefetchRanges = 1,
}) {
  if (
    !Number.isInteger(prefetchRanges) ||
    prefetchRanges < 1 ||
    prefetchRanges > REMOTE_PREFETCH_RANGES
  )
    throw Error("Replay read-ahead must be between 1 and 8 ranges");
  let received = 0,
    requested = 0;
  const ranges = [];
  const fill = () => {
    signal.throwIfAborted();
    while (ranges.length < prefetchRanges && requested < size) {
      const offset = requested;
      const length = Math.min(CHUNK_BYTES, size - offset);
      requested += length;
      ranges.push(
        (async () => {
          const bytes = await readAt(offset, length);
          signal.throwIfAborted();
          if (bytes.length !== length)
            throw Error("The source ended before its advertised size");
          received += bytes.length;
          return { bytes };
        })().catch((error) => ({ error })),
      );
    }
  };
  async function* chunks() {
    try {
      while (requested < size || ranges.length) {
        fill();
        const result = await ranges[0];
        signal.throwIfAborted();
        ranges.shift();
        // Handle speculative failures immediately, report them in stream order.
        if ("error" in result) throw result.error;
        const { bytes } = result;
        // Limit one decompressor write even for highly compressible input.
        for (let offset = 0; offset < bytes.length; offset += 65536)
          yield bytes.subarray(offset, offset + 65536);
      }
    } finally {
      ranges.length = 0;
    }
  }
  const iterator = chunks();
  const reader = compressed
    ? gzipReader(iterator)
    : new ReadableStream(
        {
          async pull(controller) {
            const { value, done } = await iterator.next();
            if (done) controller.close();
            else controller.enqueue(value);
          },
          async cancel() {
            await iterator.return();
          },
        },
        { highWaterMark: 0 },
      ).getReader();
  const onAbort = () => {
    void reader.cancel(signal.reason).catch(() => {});
  };
  signal.addEventListener("abort", onAbort, { once: true });
  let pending = new Uint8Array(),
    done = false;
  return {
    size,
    compressed,
    get received() {
      return received;
    },
    async read() {
      signal.throwIfAborted();
      const bytes = new Uint8Array(CHUNK_BYTES);
      let length = 0;
      while (length < bytes.length && !done) {
        if (!pending.length) {
          const next = await reader.read();
          signal.throwIfAborted();
          if (next.done) {
            done = true;
            signal.removeEventListener("abort", onAbort);
            break;
          }
          pending = next.value;
        }
        const take = Math.min(pending.length, bytes.length - length);
        bytes.set(pending.subarray(0, take), length);
        length += take;
        pending = pending.subarray(take);
      }
      return { bytes: bytes.subarray(0, length), eof: done };
    },
  };
}

// Automatic pipeThrough can buffer far beyond the replay consumer's demand.
// Feed at most one compressed chunk while waiting for decoded output, and drain
// that output before writing more. A pending write may span several read calls.
function gzipReader(input) {
  const decoder = new DecompressionStream("gzip");
  const writer = decoder.writable.getWriter();
  const reader = decoder.readable.getReader();
  let writing,
    closing = false,
    cancellation;
  const cancel = (reason) => {
    closing = true;
    return (cancellation ??= Promise.allSettled([
      reader.cancel(reason),
      writer.abort(reason),
      input.return(),
    ]));
  };
  const pump = async () => {
    const { value, done } = await input.next();
    if (closing) return;
    if (done) {
      closing = true;
      await writer.close();
    } else await writer.write(value);
  };
  return {
    cancel,
    async read() {
      let available = false;
      const output = reader.read().then((result) => {
        available = true;
        return result;
      });
      try {
        // Let already-buffered output settle before deciding to feed more input.
        await Promise.resolve();
        while (!available && !closing) {
          writing ??= pump().finally(() => {
            writing = undefined;
          });
          // Awaiting only the write can deadlock on decoder backpressure.
          await Promise.race([output, writing]);
        }
        return await output;
      } catch (error) {
        void cancel(error);
        throw error;
      }
    },
  };
}
