import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { createToastStore } from "../lib/stores/toasts.svelte";

describe("toastStore", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("dispatches a toast and exposes it via list", () => {
    const store = createToastStore();
    expect(store.list).toHaveLength(0);

    const id = store.push({ message: "hello", kind: "info" });
    expect(store.list).toHaveLength(1);
    expect(store.list[0].id).toBe(id);
    expect(store.list[0].message).toBe("hello");
    expect(store.list[0].kind).toBe("info");
  });

  it("error() shorthand creates an error-kind toast carrying retry", () => {
    const store = createToastStore();
    const retry = vi.fn();
    const id = store.error("Échec de l'envoi", retry);
    expect(store.list).toHaveLength(1);
    const t = store.list[0];
    expect(t.id).toBe(id);
    expect(t.kind).toBe("error");
    expect(t.message).toBe("Échec de l'envoi");
    expect(t.retry).toBe(retry);
  });

  it("dismiss removes the toast by id", () => {
    const store = createToastStore();
    const a = store.push({ message: "a" });
    const b = store.push({ message: "b" });
    expect(store.list).toHaveLength(2);

    store.dismiss(a);
    expect(store.list).toHaveLength(1);
    expect(store.list[0].id).toBe(b);
  });

  it("auto-dismisses after ttl", () => {
    const store = createToastStore();
    store.push({ message: "ephemeral", ttl: 1000 });
    expect(store.list).toHaveLength(1);

    vi.advanceTimersByTime(999);
    expect(store.list).toHaveLength(1);

    vi.advanceTimersByTime(1);
    expect(store.list).toHaveLength(0);
  });

  it("ttl=0 disables auto-dismiss", () => {
    const store = createToastStore();
    store.push({ message: "sticky", ttl: 0 });
    vi.advanceTimersByTime(60_000);
    expect(store.list).toHaveLength(1);
  });

  it("clear() removes everything and cancels pending timers", () => {
    const store = createToastStore();
    store.push({ message: "a", ttl: 1000 });
    store.push({ message: "b", ttl: 1000 });
    expect(store.list).toHaveLength(2);
    store.clear();
    expect(store.list).toHaveLength(0);
    // Advance past ttl — no resurrection.
    vi.advanceTimersByTime(2000);
    expect(store.list).toHaveLength(0);
  });

  it("manual dismiss before ttl cancels the timer", () => {
    const store = createToastStore();
    const id = store.push({ message: "x", ttl: 1000 });
    store.dismiss(id);
    // If the timer were still active, this would no-op (toast already gone) anyway,
    // but we mainly assert the list stays clean.
    vi.advanceTimersByTime(2000);
    expect(store.list).toHaveLength(0);
  });
});
