export type AsyncUnlisten = () => void;

type AsyncRegister<Args extends unknown[]> = (
  handler: (...args: Args) => void,
) => Promise<AsyncUnlisten>;

export function createAsyncListenerScope() {
  let alive = true;
  const unlisteners: AsyncUnlisten[] = [];

  return {
    listen<Args extends unknown[]>(
      register: AsyncRegister<Args>,
      handler: (...args: Args) => void,
    ): void {
      void register((...args) => {
        if (alive) handler(...args);
      }).then((unlisten) => {
        if (!alive) {
          unlisten();
          return;
        }
        unlisteners.push(unlisten);
      });
    },
    dispose(): void {
      if (!alive) return;
      alive = false;
      unlisteners.splice(0).forEach((unlisten) => unlisten());
    },
  };
}
