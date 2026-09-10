/** Native bucket quantities, in ascending price order and displayed units.
 * Mirror the existing cumulative definition: asks low-to-high, bids high-to-low.
 * This is small label metadata, never a readback of GPU buffers or pixels.
 */
export function depthProfile(
  quantities: Float64Array,
  quantityDecimals: number,
) {
  const cumulative = new Float64Array(quantities.length);
  let bid = 0,
    ask = 0;
  for (let i = 0; i < quantities.length; i += 2) {
    ask += quantities[i + 1];
    cumulative[i + 1] = ask;
    const reverse = quantities.length - 2 - i;
    bid += quantities[reverse];
    cumulative[reverse] = bid;
  }
  // The GPU operates in quantity atoms, with a minimum axis range of one atom.
  return { cumulative, maximum: Math.max(bid, ask, 10 ** -quantityDecimals) };
}
