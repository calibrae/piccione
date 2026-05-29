import { render, screen, waitFor, act, cleanup } from "@testing-library/svelte";
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { tick } from "svelte";
import type { ChatMessage, Conversation } from "../lib/types";

// ----- Tauri & dialog mocks (must be hoisted before any component import) -----

const { mockInvoke, mockListen, listeners, fileSrc } = vi.hoisted(() => {
  return {
    mockInvoke: vi.fn(),
    mockListen: vi.fn(),
    listeners: new Map<string, (event: { payload: unknown }) => void>(),
    fileSrc: (p: string) => `tauri://localhost/${encodeURIComponent(p)}`,
  };
});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: mockInvoke,
  convertFileSrc: (path: string) => fileSrc(path),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: (event: string, cb: (e: { payload: unknown }) => void) => {
    listeners.set(event, cb);
    mockListen(event);
    return Promise.resolve(() => listeners.delete(event));
  },
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

import ChatLayout from "../lib/components/ChatLayout.svelte";
import { messagingStore } from "../lib/stores/messaging.svelte";

// ----- Test fixtures -----

const conv: Conversation = {
  id: "conv-1",
  name: "Alice",
  last_message: "hi",
  last_timestamp: Date.now(),
  is_group: false,
  avatar_path: null,
};

function imageMessage(): ChatMessage {
  return {
    timestamp: Date.now(),
    sender_id: "alice",
    sender_name: "Alice",
    body: "look",
    attachments: [
      {
        id: "att-img",
        file_name: "photo.png",
        mime_type: "image/png",
        size: 12345,
        local_path: "/tmp/photo.png",
      },
    ],
    is_outgoing: false,
  };
}

function fileMessage(): ChatMessage {
  return {
    timestamp: Date.now(),
    sender_id: "alice",
    sender_name: "Alice",
    body: null,
    attachments: [
      {
        id: "att-pdf",
        file_name: "rapport.pdf",
        mime_type: "application/pdf",
        size: 2_500_000,
        local_path: "/tmp/rapport.pdf",
      },
    ],
    is_outgoing: false,
  };
}

function setupInvoke(messages: ChatMessage[]) {
  mockInvoke.mockImplementation((cmd: string) => {
    switch (cmd) {
      case "get_self_id":
        return Promise.resolve("self-uuid");
      case "get_settings":
        return Promise.resolve({
          read_receipts: true,
          typing_indicators: true,
          theme: "auto",
        });
      case "get_conversations":
        return Promise.resolve([conv]);
      case "get_messages":
        return Promise.resolve(messages);
      case "send_message":
      case "send_message_with_attachments":
      case "send_to_recipient":
        return Promise.resolve(undefined);
      default:
        return Promise.resolve(undefined);
    }
  });
}

async function selectConversation() {
  // Pick the sidebar button (avoid the active-header <h2>Alice</h2>).
  const matches = await screen.findAllByText("Alice");
  const button = matches.map((el) => el.closest("button")).find(Boolean) as HTMLButtonElement;
  expect(button).toBeTruthy();
  await act(async () => {
    button.click();
    await tick();
  });
}

beforeEach(async () => {
  mockInvoke.mockReset();
  mockListen.mockReset();
  // jsdom has no matchMedia; applyTheme("auto") needs it.
  if (!window.matchMedia) {
    window.matchMedia = vi.fn().mockImplementation((query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      addListener: vi.fn(),
      removeListener: vi.fn(),
      dispatchEvent: vi.fn(),
    })) as unknown as typeof window.matchMedia;
  }
  // The store no longer registers listeners at module load — App.svelte
  // does that on mount. In ChatLayout-only tests we have to wire them up.
  await messagingStore.initListeners();
});

afterEach(() => {
  cleanup();
});

describe("ChatLayout attachments", () => {
  it("renders an <img> for image-mime attachments", async () => {
    setupInvoke([imageMessage()]);
    render(ChatLayout);
    await selectConversation();

    await waitFor(() => {
      const img = screen.getByAltText("photo.png") as HTMLImageElement;
      expect(img).toBeInTheDocument();
      expect(img.tagName).toBe("IMG");
      expect(img.src).toContain("photo.png");
    });
  });

  it("renders a generic file row for non-image attachments", async () => {
    setupInvoke([fileMessage()]);
    render(ChatLayout);
    await selectConversation();

    await waitFor(() => {
      const row = screen.getByTestId("attachment-file");
      expect(row).toBeInTheDocument();
      expect(row.textContent).toContain("rapport.pdf");
      expect(row.textContent).toContain("PDF");
      // No <img> rendered for the file row.
      expect(screen.queryByAltText("rapport.pdf")).toBeNull();
    });
  });
});

describe("ChatLayout scroll-to-bottom reactivity", () => {
  it("scrolls to bottom when the message list grows", async () => {
    setupInvoke([imageMessage()]);

    // Stub scrollTop / scrollHeight on the bound container.
    let scrollTopSet = 0;
    const origScrollTop = Object.getOwnPropertyDescriptor(
      HTMLElement.prototype,
      "scrollTop"
    );
    const origScrollHeight = Object.getOwnPropertyDescriptor(
      HTMLElement.prototype,
      "scrollHeight"
    );
    Object.defineProperty(HTMLElement.prototype, "scrollTop", {
      configurable: true,
      get(this: HTMLElement) {
        return (this as unknown as { _scrollTop?: number })._scrollTop ?? 0;
      },
      set(this: HTMLElement, v: number) {
        scrollTopSet = v;
        (this as unknown as { _scrollTop?: number })._scrollTop = v;
      },
    });
    Object.defineProperty(HTMLElement.prototype, "scrollHeight", {
      configurable: true,
      get() {
        return 9999;
      },
    });

    // Use immediate rAF so the $effect commits inside the test tick.
    const origRaf = window.requestAnimationFrame;
    (window as unknown as {
      requestAnimationFrame: (cb: FrameRequestCallback) => number;
    }).requestAnimationFrame = (cb: FrameRequestCallback) => {
      cb(0);
      return 0;
    };

    try {
      render(ChatLayout);
      await selectConversation();

      await waitFor(() => {
        expect(screen.getByTestId("messages-container")).toBeInTheDocument();
      });

      // Reset and dispatch a new-message event simulating an inbound message.
      scrollTopSet = 0;
      const cb = listeners.get("new-message");
      expect(cb).toBeDefined();
      await act(async () => {
        cb!({
          payload: {
            conversation_id: conv.id,
            message: { ...imageMessage(), timestamp: Date.now() + 1, body: "next" },
          },
        });
        await tick();
      });

      await waitFor(() => {
        expect(scrollTopSet).toBe(9999);
      });
    } finally {
      window.requestAnimationFrame = origRaf;
      if (origScrollTop) {
        Object.defineProperty(HTMLElement.prototype, "scrollTop", origScrollTop);
      }
      if (origScrollHeight) {
        Object.defineProperty(HTMLElement.prototype, "scrollHeight", origScrollHeight);
      }
    }
  });
});
