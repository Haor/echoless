import { describe, expect, it } from "vitest";
import { createAsyncListenerScope } from "./asyncListener";

function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

function delayedRegistry<T>() {
  const ready = deferred();
  const handlers = new Set<(event: T) => void>();
  let unlistenCalls = 0;

  return {
    handlers,
    get unlistenCalls() {
      return unlistenCalls;
    },
    register(handler: (event: T) => void) {
      handlers.add(handler);
      return ready.promise.then(() => () => {
        handlers.delete(handler);
        unlistenCalls += 1;
      });
    },
    emit(event: T) {
      handlers.forEach((handler) => handler(event));
    },
    async resolveRegistrations() {
      ready.resolve();
      await ready.promise;
      await Promise.resolve();
    },
  };
}

describe("createAsyncListenerScope", () => {
  it("unlistens a delayed registration immediately after early cleanup", async () => {
    const registry = delayedRegistry<string>();
    const received: string[] = [];
    const scope = createAsyncListenerScope();

    scope.listen(registry.register, (event) => received.push(event));
    scope.dispose();

    registry.emit("stale-before-resolve");
    expect(received).toEqual([]);

    await registry.resolveRegistrations();

    expect(registry.handlers.size).toBe(0);
    expect(registry.unlistenCalls).toBe(1);
  });

  it("keeps only the remounted handler effective across StrictMode cleanup", async () => {
    const registry = delayedRegistry<number>();
    const received: number[] = [];
    const mount = () => {
      const scope = createAsyncListenerScope();
      scope.listen(registry.register, (event) => received.push(event));
      return () => scope.dispose();
    };

    const cleanupFirstMount = mount();
    cleanupFirstMount();
    const cleanupSecondMount = mount();

    registry.emit(1);
    expect(received).toEqual([1]);

    await registry.resolveRegistrations();

    expect(registry.handlers.size).toBe(1);
    expect(registry.unlistenCalls).toBe(1);
    registry.emit(2);
    expect(received).toEqual([1, 2]);

    cleanupSecondMount();
    expect(registry.handlers.size).toBe(0);
    expect(registry.unlistenCalls).toBe(2);
  });
});
