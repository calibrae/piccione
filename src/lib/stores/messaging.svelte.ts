import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Conversation, ChatMessage } from "../types";

// --- Modifier event payloads (mirror src-tauri/src/messaging/types.rs) ---

export type ReceiptKind = "delivered" | "read" | "viewed";
export type TypingActionKind = "started" | "stopped";

export interface ReceiptPayload {
  chat_id: string;
  message_ids: string[];
  type: ReceiptKind;
  timestamp: number;
}

export interface TypingPayload {
  chat_id: string;
  sender_id: string;
  action: TypingActionKind;
}

export interface ReactionPayload {
  chat_id: string;
  target_message_id: string;
  emoji: string;
  sender_id: string;
  remove: boolean;
}

export interface EditPayload {
  chat_id: string;
  message_id: string;
  new_text: string;
  edited_at: number;
}

export interface DeletePayload {
  chat_id: string;
  message_id: string;
}

export function createMessagingStore() {
  let conversations = $state<Conversation[]>([]);
  let activeConversationId = $state<string | null>(null);
  let messages = $state<Map<string, ChatMessage[]>>(new Map());
  let selfId = $state<string | null>(null);

  // Modifier mirrors. The UI renderer is a separate swimlane — for now we
  // just keep the latest known state per conversation/message so that swarm
  // can wire the components without re-doing the IPC layer.
  // chat_id -> message_id -> { kind, ts }
  let receipts = $state<Map<string, Map<string, ReceiptPayload>>>(new Map());
  // chat_id -> sender_id -> action
  let typing = $state<Map<string, Map<string, TypingActionKind>>>(new Map());
  // chat_id -> target_message_id -> sender_id -> emoji (or removed)
  let reactions = $state<
    Map<string, Map<string, Map<string, string | null>>>
  >(new Map());
  // message_id -> new text
  let edits = $state<Map<string, EditPayload>>(new Map());
  // message_id -> tombstone marker
  let deletions = $state<Set<string>>(new Set());

  // Listen for new messages
  listen<{ conversation_id: string; message: ChatMessage }>(
    "new-message",
    (event) => {
      const { conversation_id, message } = event.payload;
      const existing = messages.get(conversation_id) ?? [];
      messages.set(conversation_id, [...existing, message]);
      messages = new Map(messages); // trigger reactivity
    }
  );

  // Listen for conversation updates (new messages, contacts sync)
  listen("conversations-updated", () => {
    loadConversations();
  });

  // ---- Modifier listeners ----

  listen<ReceiptPayload>("read-receipt", (event) => {
    const p = event.payload;
    const perChat = receipts.get(p.chat_id) ?? new Map();
    for (const mid of p.message_ids) {
      perChat.set(mid, p);
    }
    receipts.set(p.chat_id, perChat);
    receipts = new Map(receipts);
  });

  listen<TypingPayload>("typing-indicator", (event) => {
    const p = event.payload;
    const perChat = typing.get(p.chat_id) ?? new Map();
    if (p.action === "stopped") {
      perChat.delete(p.sender_id);
    } else {
      perChat.set(p.sender_id, p.action);
    }
    if (perChat.size === 0) {
      typing.delete(p.chat_id);
    } else {
      typing.set(p.chat_id, perChat);
    }
    typing = new Map(typing);
  });

  listen<ReactionPayload>("reaction", (event) => {
    const p = event.payload;
    const perChat = reactions.get(p.chat_id) ?? new Map();
    const perMsg = perChat.get(p.target_message_id) ?? new Map();
    if (p.remove) {
      perMsg.delete(p.sender_id);
    } else {
      perMsg.set(p.sender_id, p.emoji);
    }
    if (perMsg.size === 0) {
      perChat.delete(p.target_message_id);
    } else {
      perChat.set(p.target_message_id, perMsg);
    }
    reactions.set(p.chat_id, perChat);
    reactions = new Map(reactions);
  });

  listen<EditPayload>("message-edited", (event) => {
    const p = event.payload;
    edits.set(p.message_id, p);
    edits = new Map(edits);
  });

  listen<DeletePayload>("message-deleted", (event) => {
    const p = event.payload;
    deletions.add(p.message_id);
    deletions = new Set(deletions);
  });

  async function loadConversations() {
    try {
      conversations = await invoke<Conversation[]>("get_conversations");
    } catch (e) {
      console.error("Failed to load conversations:", e);
    }
  }

  async function loadMessages(conversationId: string) {
    try {
      const msgs = await invoke<ChatMessage[]>("get_messages", {
        conversationId,
      });
      messages.set(conversationId, msgs);
      messages = new Map(messages);
    } catch (e) {
      console.error("Failed to load messages:", e);
    }
  }

  async function sendMessage(conversationId: string, body: string) {
    try {
      await invoke("send_message", { conversationId, body });
      // Optimistically add the sent message
      const now = Date.now();
      const existing = messages.get(conversationId) ?? [];
      messages.set(conversationId, [
        ...existing,
        {
          timestamp: now,
          sender_id: selfId ?? "",
          sender_name: "You",
          body,
          attachments: [],
          is_outgoing: true,
        },
      ]);
      messages = new Map(messages);
      await loadConversations();
    } catch (e) {
      console.error("Failed to send message:", e);
    }
  }

  async function sendMessageWithAttachments(
    conversationId: string,
    body: string,
    filePaths: string[]
  ) {
    try {
      await invoke("send_message_with_attachments", {
        conversationId,
        body,
        filePaths,
      });
      await loadConversations();
      await loadMessages(conversationId);
    } catch (e) {
      console.error("Failed to send with attachments:", e);
      throw e;
    }
  }

  async function loadSelfId() {
    try {
      selfId = await invoke<string | null>("get_self_id");
    } catch (e) {
      console.error("Failed to get self ID:", e);
    }
  }

  return {
    get conversations() {
      return conversations;
    },
    get activeConversationId() {
      return activeConversationId;
    },
    set activeConversationId(id: string | null) {
      activeConversationId = id;
      if (id) {
        loadMessages(id);
      }
    },
    get selfId() {
      return selfId;
    },
    get receipts() {
      return receipts;
    },
    get typing() {
      return typing;
    },
    get reactions() {
      return reactions;
    },
    get edits() {
      return edits;
    },
    get deletions() {
      return deletions;
    },
    getMessages(conversationId: string): ChatMessage[] {
      return messages.get(conversationId) ?? [];
    },
    loadConversations,
    loadMessages,
    sendMessage,
    sendMessageWithAttachments,
    loadSelfId,
  };
}

export const messagingStore = createMessagingStore();
