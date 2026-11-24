/** Rejects with `onTimeout()` if `work` has not settled in `ms`. The work
 * itself is not cancelled (the SDK exposes no abort), only abandoned. */
export function withTimeout<T>(work: Promise<T>, ms: number, onTimeout: () => Error): Promise<T> {
  let timer: NodeJS.Timeout | undefined;
  const timeout = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(onTimeout()), ms);
  });
  // Swallow a late rejection from the abandoned work so it cannot crash the process.
  work.catch(() => undefined);
  return Promise.race([work, timeout]).finally(() => clearTimeout(timer));
}
