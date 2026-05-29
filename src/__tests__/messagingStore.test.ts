import { describe, it, expect, vi, beforeEach } from "vitest";

// Hoist mocks before importing the store.
const { mockInvoke, listenCalls, unlistenCalls, listeners } = vi.hoisted(() => ({
  mockInvoke: vi.fn().mockResolvedValue(undefined),
  listenCalls: [] as string[],
  unlistenCalls: [] as string[],
  listeners: new Map<string, (event: { payload: unknown }) => void>(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: mockInvoke,
  convertFileSrc: (p: string) => `tauri://localhost/${p}`,
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (event: string, cb: (e: { payload: unknown }) => void) => {
    listenCalls.push(event);
    listeners.set(event, cb);
    return () => {
      unlistenCalls.push(event);
      listeners.delete(event);
    };
  }),
}));

import { createMessagingStore } from "../lib/stores/messaging.svelte";

describe("messagingStore.initListeners", () => {
  beforeEach(() => {
    listenCalls.length = 0;
    unlistenCalls.length = 0;
    listeners.clear();
    mockInvoke.mockClear();
  });

  it("registers exactly one listener per event", async () => {
    const store = createMessagingStore();
    await store.initListeners();
    // 8 IPC events: new-message, conversations-updated, read-receipt,
    // typing-indicator, reaction, poll-vote, message-edited, message-deleted.
    const expected = new Set([
      "new-message",
      "conversations-updated",
      "read-receipt",
      "typing-indicator",
      "reaction",
      "poll-vote",
      "message-edited",
      "message-deleted",
    ]);
    expect(new Set(listenCalls)).toEqual(expected);
    expect(listenCalls.length).toBe(8);
    expect(store._listenerCount).toBe(8);
  });

  it("is idempotent: a second initListeners call does not double-subscribe", async () => {
    const store = createMessagingStore();
    await store.initListeners();
    const firstCount = listenCalls.length;
    await store.initListeners();
    expect(listenCalls.length).toBe(firstCount);
    expect(store._listenerCount).toBe(8);
  });

  it("returns a teardown that unsubscribes every listener", async () => {
    const store = createMessagingStore();
    const teardown = await store.initListeners();
    expect(store._listenerCount).toBe(8);
    await teardown();
    expect(unlistenCalls.length).toBe(8);
    expect(store._listenerCount).toBe(0);
  });

  it("can be re-armed after teardown (HMR / restart path)", async () => {
    const store = createMessagingStore();
    const teardown = await store.initListeners();
    await teardown();
    listenCalls.length = 0;
    await store.initListeners();
    expect(listenCalls.length).toBe(8);
    expect(store._listenerCount).toBe(8);
  });
});

describe("messagingStore.sendToRecipient", () => {
  beforeEach(() => {
    listenCalls.length = 0;
    unlistenCalls.length = 0;
    listeners.clear();
    mockInvoke.mockReset();
  });

  it("invokes send_to_recipient and resolves on success", async () => {
    mockInvoke.mockResolvedValue(undefined);
    const store = createMessagingStore();
    await store.sendToRecipient("uuid-1", "hi");
    expect(mockInvoke).toHaveBeenCalledWith("send_to_recipient", {
      recipientId: "uuid-1",
      body: "hi",
    });
  });

  it("rethrows on failure so the caller can keep the form open", async () => {
    mockInvoke.mockRejectedValue("boom");
    const store = createMessagingStore();
    await expect(store.sendToRecipient("uuid-1", "hi")).rejects.toBe("boom");
  });
});
