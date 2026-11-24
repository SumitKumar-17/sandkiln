/** Output bounding. Everything returned to the model passes through here so
 * one noisy command or large file cannot flood its context window. */

const enc = new TextEncoder();
const dec = new TextDecoder("utf-8");

export interface Truncated {
  text: string;
  truncated: boolean;
  /** Size of the original text in UTF-8 bytes. */
  totalBytes: number;
  omittedBytes: number;
}

/** Largest index <= `end` that does not split a UTF-8 sequence in `bytes`. */
function safeEnd(bytes: Uint8Array, end: number): number {
  let i = Math.min(end, bytes.length);
  // A continuation byte (10xxxxxx) at `i` means the cut lands mid-character.
  while (i > 0 && i < bytes.length && (bytes[i]! & 0xc0) === 0x80) i--;
  return i;
}

/** Smallest index >= `start` that begins a UTF-8 sequence. */
function safeStart(bytes: Uint8Array, start: number): number {
  let i = Math.max(start, 0);
  while (i < bytes.length && (bytes[i]! & 0xc0) === 0x80) i++;
  return i;
}

/** Keeps the head and tail of `text` within `maxBytes` and marks the gap.
 * Both ends matter for command output: the start shows what ran, the end
 * usually holds the error. */
export function truncateText(text: string, maxBytes: number): Truncated {
  const bytes = enc.encode(text);
  if (bytes.length <= maxBytes) {
    return { text, truncated: false, totalBytes: bytes.length, omittedBytes: 0 };
  }
  const headBudget = Math.ceil(maxBytes / 2);
  const tailBudget = maxBytes - headBudget;
  const headEnd = safeEnd(bytes, headBudget);
  const tailStart = safeStart(bytes, bytes.length - tailBudget);
  const omitted = tailStart - headEnd;
  const marker = `\n[... ${omitted} bytes omitted of ${bytes.length} total ...]\n`;
  return {
    text: dec.decode(bytes.subarray(0, headEnd)) + marker + dec.decode(bytes.subarray(tailStart)),
    truncated: true,
    totalBytes: bytes.length,
    omittedBytes: omitted,
  };
}

export interface DecodedFile {
  encoding: "utf8" | "base64";
  content: string;
  totalBytes: number;
  returnedBytes: number;
  truncated: boolean;
}

/** Turns file bytes into a model-safe payload: UTF-8 text when the bytes
 * are valid text, base64 otherwise, cut to `maxBytes` of source data. */
export function decodeFile(bytes: Uint8Array, maxBytes: number): DecodedFile {
  const total = bytes.length;
  const truncated = total > maxBytes;
  let slice = truncated ? bytes.subarray(0, safeEnd(bytes, maxBytes)) : bytes;
  let isText = !slice.includes(0);
  if (isText) {
    try {
      new TextDecoder("utf-8", { fatal: true }).decode(slice);
    } catch {
      isText = false;
    }
  }
  if (isText) {
    return { encoding: "utf8", content: dec.decode(slice), totalBytes: total, returnedBytes: slice.length, truncated };
  }
  // Binary content has no character boundaries, so cut at exactly maxBytes.
  slice = truncated ? bytes.subarray(0, maxBytes) : bytes;
  return {
    encoding: "base64",
    content: Buffer.from(slice).toString("base64"),
    totalBytes: total,
    returnedBytes: slice.length,
    truncated,
  };
}
