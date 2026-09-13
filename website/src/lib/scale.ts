// Shared log axis for the measured-latency readout.
//
// The readout plots every figure on one axis so the two orders of magnitude
// between an exec round-trip and a full create are visible rather than merely
// stated. A linear axis can't hold 0.225ms and 211ms at once, hence log.
//
// Positions are derived from the measurements themselves — nothing here
// hard-codes a percentage, so correcting a figure moves its mark too.

export const AXIS_DECADES = [0.1, 1, 10, 100, 1000] as const;

const MIN = AXIS_DECADES[0];
const MAX = AXIS_DECADES[AXIS_DECADES.length - 1];
const SPAN = Math.log10(MAX) - Math.log10(MIN);

/** Position of a millisecond value on the axis, 0–100. */
export function pos(ms: number): number {
  return ((Math.log10(ms) - Math.log10(MIN)) / SPAN) * 100;
}

export const ticks = AXIS_DECADES.map((ms) => ({
  at: pos(ms),
  label: ms < 1 ? `${ms * 1000}µs` : ms >= 1000 ? `${ms / 1000}s` : `${ms}ms`,
}));

export interface Measurement {
  label: string;
  /** Low and high of the measured range, in milliseconds. Equal when a run
   *  recorded a single figure rather than a range. */
  lo: number;
  hi: number;
  /** Exactly as it should be printed — never a rounded midpoint of a range. */
  value: string;
  unit: string;
  source: string;
}

export function markStyle(m: Measurement): string {
  const a = pos(m.lo);
  const b = pos(m.hi);
  return `left:${a.toFixed(2)}%;width:${Math.max(b - a, 0).toFixed(2)}%`;
}
