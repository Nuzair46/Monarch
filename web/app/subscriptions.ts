/** Own asynchronous registrations even when their effect has already unmounted. */
export function subscriptions(onError: (error: unknown) => void) {
  let disposed = false;
  const cleanups: Array<() => void> = [];
  return {
    add(registration: Promise<() => void>): void {
      void registration.then((cleanup) => {
        if (disposed) cleanup();
        else cleanups.push(cleanup);
      }).catch((error) => { if (!disposed) onError(error); });
    },
    dispose(): void {
      disposed = true;
      for (const cleanup of cleanups.splice(0)) cleanup();
    },
  };
}
