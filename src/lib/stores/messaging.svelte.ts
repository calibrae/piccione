import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import type { Conversation, ChatMessage } from "../types";
import { toastStore } from "./toasts.svelte";

export interface QuoteInput {
  id: number;
  author_uuid: string;
  text: string;
}

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

export interface PollVotePayload {
  chat_id: string;
  poll_id: string;
  voter_id: string;
  option_indexes: number[];
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
  let notifyOk = false;
  // Per-conversation mute (persisted). Muted threads suppress notifications.
  let muted = $state<Set<string>>(loadMuted());

  function loadMuted(): Set<string> {
    try {
      const raw = localStorage.getItem("piccione.muted");
      return new Set(raw ? (JSON.parse(raw) as string[]) : []);
    } catch {
      return new Set();
    }
  }
  function persistMuted() {
    try {
      localStorage.setItem("piccione.muted", JSON.stringify([...muted]));
    } catch {
      /* ignore */
    }
  }
  // Per-conversation unread counts (in-memory; resets on restart).
  let unread = $state<Map<string, number>>(new Map());

  async function ensureNotifyPermission() {
    try {
      notifyOk = await isPermissionGranted();
      if (!notifyOk) notifyOk = (await requestPermission()) === "granted";
    } catch {
      notifyOk = false;
    }
  }

  // Desktop notification for an inbound message when the window is not focused.
  async function notifyInbound(conversationId: string, message: ChatMessage) {
    if (message.is_outgoing) return;
    if (muted.has(conversationId)) return;
    if (typeof document !== "undefined" && document.hasFocus()) return;
    if (!notifyOk) await ensureNotifyPermission();
    if (!notifyOk) return;
    const convo = conversations.find((c) => c.id === conversationId);
    const title = convo?.name ?? message.sender_name ?? "Nouveau message";
    const body = message.body
      ? message.body.slice(0, 140)
      : message.attachments?.length
        ? "📎 Pièce jointe"
        : "";
    try {
      sendNotification({ title, body });
    } catch (e) {
      console.error("notify failed:", e);
    }
  }

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
  // chat_id -> poll_id -> voter_id -> option indexes
  let pollVotes = $state<Map<string, Map<string, Map<string, number[]>>>>(new Map());

  // Cleanup handles for IPC listeners. Populated by `initListeners()`,
  // drained by the returned teardown to prevent pile-ups on HMR / app restart.
  let unsubscribers: UnlistenFn[] = [];
  let initialized = false;

  /**
   * Register Tauri event listeners. Idempotent: calling twice is a no-op.
   * Returns a teardown function that removes every listener and resets the
   * "initialized" flag so the store can be re-armed (test-only path).
   */
  async function initListeners(): Promise<() => Promise<void>> {
    if (initialized) {
      return teardown;
    }
    initialized = true;

    const subs = await Promise.all([
      listen<{ conversation_id: string; message: ChatMessage }>(
        "new-message",
        (event) => {
          const { conversation_id, message } = event.payload;
          const existing = messages.get(conversation_id) ?? [];
          messages.set(conversation_id, [...existing, message]);
          messages = new Map(messages);
          if (!message.is_outgoing && conversation_id !== activeConversationId) {
            unread.set(conversation_id, (unread.get(conversation_id) ?? 0) + 1);
            unread = new Map(unread);
          }
          void notifyInbound(conversation_id, message);
        }
      ),
      listen("conversations-updated", () => {
        loadConversations();
      }),
      listen<ReceiptPayload>("read-receipt", (event) => {
        const p = event.payload;
        const perChat = receipts.get(p.chat_id) ?? new Map();
        for (const mid of p.message_ids) {
          perChat.set(mid, p);
        }
        receipts.set(p.chat_id, perChat);
        receipts = new Map(receipts);
      }),
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
      }),
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
      }),
      listen<PollVotePayload>("poll-vote", (event) => {
        const p = event.payload;
        const perChat = pollVotes.get(p.chat_id) ?? new Map();
        const perPoll = perChat.get(p.poll_id) ?? new Map();
        perPoll.set(p.voter_id, p.option_indexes);
        perChat.set(p.poll_id, perPoll);
        pollVotes.set(p.chat_id, perChat);
        pollVotes = new Map(pollVotes);
      }),
      listen<EditPayload>("message-edited", (event) => {
        const p = event.payload;
        edits.set(p.message_id, p);
        edits = new Map(edits);
      }),
      listen<DeletePayload>("message-deleted", (event) => {
        const p = event.payload;
        deletions.add(p.message_id);
        deletions = new Set(deletions);
      }),
    ]);
    unsubscribers = subs;
    return teardown;
  }

  async function teardown(): Promise<void> {
    for (const fn of unsubscribers) {
      try {
        fn();
      } catch (e) {
        console.error("listener teardown failed:", e);
      }
    }
    unsubscribers = [];
    initialized = false;
  }

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
    filePaths: string[],
    quote?: QuoteInput
  ) {
    try {
      await invoke("send_message_with_attachments", {
        conversationId,
        body,
        filePaths,
        quote: quote ?? null,
      });
      await loadConversations();
      await loadMessages(conversationId);
    } catch (e) {
      console.error("Failed to send with attachments:", e);
      toastStore.error("Échec de l'envoi", () =>
        sendMessageWithAttachments(conversationId, body, filePaths, quote)
      );
      throw e;
    }
  }

  async function sendToRecipient(recipientId: string, body: string) {
    try {
      await invoke("send_to_recipient", { recipientId, body });
    } catch (e) {
      console.error("Failed to send to recipient:", e);
      toastStore.error("Échec de l'envoi", () =>
        sendToRecipient(recipientId, body)
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

  async function sendReaction(
    conversationId: string,
    targetAuthorUuid: string,
    targetTimestamp: number,
    emoji: string,
    remove: boolean
  ) {
    try {
      await invoke("send_reaction", {
        conversationId,
        targetAuthorUuid,
        targetTimestamp,
        emoji,
        remove,
      });
      // Optimistic local update so the chip flips instantly.
      const perChat = reactions.get(conversationId) ?? new Map();
      const key = String(targetTimestamp);
      const perMsg = perChat.get(key) ?? new Map();
      const me = selfId ?? "";
      if (remove) perMsg.delete(me);
      else perMsg.set(me, emoji);
      if (perMsg.size === 0) perChat.delete(key);
      else perChat.set(key, perMsg);
      reactions.set(conversationId, perChat);
      reactions = new Map(reactions);
    } catch (e) {
      console.error("send_reaction failed:", e);
    }
  }

  async function deleteForEveryone(conversationId: string, targetTimestamp: number) {
    try {
      await invoke("delete_for_everyone", { conversationId, targetTimestamp });
      // Optimistically hide locally.
      deletions.add(String(targetTimestamp));
      deletions = new Set(deletions);
    } catch (e) {
      console.error("delete_for_everyone failed:", e);
      toastStore.error("Échec de la suppression");
    }
  }

  function isMuted(conversationId: string): boolean {
    return muted.has(conversationId);
  }
  function toggleMute(conversationId: string) {
    if (muted.has(conversationId)) muted.delete(conversationId);
    else muted.add(conversationId);
    muted = new Set(muted);
    persistMuted();
  }

  async function votePoll(
    conversationId: string,
    pollAuthorUuid: string,
    pollTimestamp: number,
    optionIndexes: number[]
  ) {
    try {
      await invoke("vote_poll", {
        conversationId,
        targetAuthorUuid: pollAuthorUuid,
        targetTimestamp: pollTimestamp,
        optionIndexes,
      });
      // Optimistic local tally.
      const perChat = pollVotes.get(conversationId) ?? new Map();
      const perPoll = perChat.get(String(pollTimestamp)) ?? new Map();
      perPoll.set(selfId ?? "", optionIndexes);
      perChat.set(String(pollTimestamp), perPoll);
      pollVotes.set(conversationId, perChat);
      pollVotes = new Map(pollVotes);
    } catch (e) {
      console.error("vote_poll failed:", e);
    }
  }

  async function fetchProfile(uuid: string) {
    try {
      const name = await invoke<string | null>("fetch_profile", { uuid });
      if (name) await loadConversations();
    } catch (e) {
      console.error("fetch_profile failed:", e);
    }
  }

  function markRead(conversationId: string) {
    if (unread.has(conversationId)) {
      unread.delete(conversationId);
      unread = new Map(unread);
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
    get unread() {
      return unread;
    },
    markRead,
    get muted() {
      return muted;
    },
    isMuted,
    toggleMute,
    fetchProfile,
    get pollVotes() {
      return pollVotes;
    },
    votePoll,
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
    /** Test-only: introspection of the listener registry. */
    get _listenerCount() {
      return unsubscribers.length;
    },
    getMessages(conversationId: string): ChatMessage[] {
      const list = messages.get(conversationId) ?? [];
      if (edits.size === 0) return list;
      // Apply any received edits to the displayed body.
      return list.map((m) => {
        const e = edits.get(String(m.timestamp));
        return e ? { ...m, body: e.new_text, edited: true } : m;
      });
    },
    initListeners,
    loadConversations,
    loadMessages,
    sendMessage,
    sendMessageWithAttachments,
    sendToRecipient,
    loadSelfId,
    sendReaction,
    deleteForEveryone,
  };
}

export const messagingStore = createMessagingStore();
