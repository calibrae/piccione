import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Conversation, ChatMessage } from "../types";
import { toastStore } from "./toasts.svelte";

export function createMessagingStore() {
  let conversations = $state<Conversation[]>([]);
  let activeConversationId = $state<string | null>(null);
  let messages = $state<Map<string, ChatMessage[]>>(new Map());
  let selfId = $state<string | null>(null);

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
      toastStore.error("Échec de l'envoi", () =>
        sendMessage(conversationId, body)
      );
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
      toastStore.error("Échec de l'envoi", () =>
        sendMessageWithAttachments(conversationId, body, filePaths)
      );
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
