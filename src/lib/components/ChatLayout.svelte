<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { convertFileSrc } from "@tauri-apps/api/core";
  import { open } from "@tauri-apps/plugin-dialog";
  import { messagingStore } from "../stores/messaging.svelte";

  let inputText = $state("");
  let messagesContainer: HTMLDivElement;
  let showNewMessage = $state(false);
  let newRecipient = $state("");
  let newMessageText = $state("");
  let sendError = $state("");
  let pendingFiles = $state<string[]>([]);
  let sending = $state(false);

  onMount(async () => {
    await messagingStore.loadSelfId();
    await messagingStore.loadConversations();

    // Poll for conversations while empty (contacts sync may take time)
    const interval = setInterval(async () => {
      await messagingStore.loadConversations();
    }, 5000);

    return () => clearInterval(interval);
  });

  function selectConversation(id: string) {
    messagingStore.activeConversationId = id;
    showNewMessage = false;
  }

  async function handleSend() {
    const text = inputText.trim();
    if ((!text && pendingFiles.length === 0) || !messagingStore.activeConversationId) return;
    const body = text;
    const files = [...pendingFiles];
    const convId = messagingStore.activeConversationId;
    inputText = "";
    pendingFiles = [];

    // Don't block the UI — fire and forget
    if (files.length > 0) {
      messagingStore.sendMessageWithAttachments(convId, body, files);
    } else {
      messagingStore.sendMessage(convId, body);
    }

    requestAnimationFrame(() => {
      if (messagesContainer) {
        messagesContainer.scrollTop = messagesContainer.scrollHeight;
      }
    });
  }

  async function handleAttachFile() {
    try {
      const result = await open({
        multiple: true,
        filters: [
          { name: "Images", extensions: ["png", "jpg", "jpeg", "gif", "webp"] },
          { name: "All Files", extensions: ["*"] },
        ],
      });
      if (result) {
        const paths = Array.isArray(result) ? result : [result];
        pendingFiles = [...pendingFiles, ...paths.map(p => typeof p === 'string' ? p : p.path)];
      }
    } catch (e) {
      console.error("File picker error:", e);
    }
  }

  function removePendingFile(index: number) {
    pendingFiles = pendingFiles.filter((_, i) => i !== index);
  }

  async function handleNewMessageSend() {
    const recipient = newRecipient.trim();
    const text = newMessageText.trim();
    if (!recipient || !text) return;
    sendError = "";

    try {
      await invoke("send_to_recipient", { recipientId: recipient, body: text });
      // Switch to this conversation
      messagingStore.activeConversationId = recipient;
      showNewMessage = false;
      newRecipient = "";
      newMessageText = "";
      // Refresh conversations
      await messagingStore.loadConversations();
      await messagingStore.loadMessages(recipient);
    } catch (e) {
      sendError = String(e);
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  }

  function handleNewMsgKeydown(e: KeyboardEvent) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleNewMessageSend();
    }
  }

  function formatSize(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  }

  function formatTime(timestamp: number): string {
    if (!timestamp) return "";
    const date = new Date(timestamp);
    const now = new Date();
    const isToday = date.toDateString() === now.toDateString();
    if (isToday) {
      return date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
    }
    return date.toLocaleDateString([], { month: "short", day: "numeric" });
  }

  let activeMessages = $derived(
    messagingStore.activeConversationId
      ? messagingStore.getMessages(messagingStore.activeConversationId)
      : []
  );

  // Auto-scroll when messages change
  $effect(() => {
    if (activeMessages.length > 0 && messagesContainer) {
      requestAnimationFrame(() => {
        messagesContainer.scrollTop = messagesContainer.scrollHeight;
      });
    }
  });

  let activeConversation = $derived(
    messagingStore.conversations.find(
      (c) => c.id === messagingStore.activeConversationId
    )
  );
</script>

<div class="layout">
  <aside class="sidebar">
    <div class="sidebar-header">
      <h1>SignalUI</h1>
      <button class="new-msg-btn" onclick={() => (showNewMessage = true)} title="New Message">
        +
      </button>
    </div>
    <div class="conversations">
      {#if messagingStore.conversations.length === 0}
        <div class="empty-conversations">
          <div class="sync-indicator">
            <div class="spinner-small"></div>
            <p>Syncing contacts...</p>
            <p class="sync-hint">Send yourself a message to start</p>
          </div>
        </div>
      {:else}
        {#each messagingStore.conversations as convo}
          <button
            class="conversation"
            class:active={messagingStore.activeConversationId === convo.id}
            onclick={() => selectConversation(convo.id)}
          >
            <div class="avatar">{convo.name[0]?.toUpperCase() ?? "?"}</div>
            <div class="convo-info">
              <div class="convo-top">
                <span class="convo-name">{convo.name}</span>
                <span class="convo-time">{formatTime(convo.last_timestamp)}</span>
              </div>
              <div class="convo-last">{convo.last_message ?? ""}</div>
            </div>
          </button>
        {/each}
      {/if}
    </div>
  </aside>

  <main class="chat-area">
    {#if showNewMessage}
      <div class="chat-header">
        <h2>New Message</h2>
      </div>
      <div class="new-message-form">
        <div class="form-field">
          <label for="recipient">Recipient UUID</label>
          <input
            id="recipient"
            type="text"
            placeholder="Enter contact UUID (e.g. from Signal)"
            bind:value={newRecipient}
          />
          {#if messagingStore.selfId}
            <button class="self-btn" onclick={() => (newRecipient = messagingStore.selfId ?? "")}>
              Note to Self
            </button>
          {/if}
        </div>
        <div class="form-field">
          <label for="new-msg">Message</label>
          <input
            id="new-msg"
            type="text"
            placeholder="Type your message..."
            bind:value={newMessageText}
            onkeydown={handleNewMsgKeydown}
          />
        </div>
        {#if sendError}
          <p class="send-error">{sendError}</p>
        {/if}
        <div class="form-actions">
          <button class="secondary-btn" onclick={() => (showNewMessage = false)}>Cancel</button>
          <button class="primary-btn" onclick={handleNewMessageSend}>Send</button>
        </div>
      </div>

    {:else if activeConversation}
      <div class="chat-header">
        <h2>{activeConversation.name}</h2>
      </div>
      <div class="messages" bind:this={messagesContainer}>
        {#each activeMessages as msg}
          <div class="message" class:outgoing={msg.is_outgoing}>
            <div class="bubble">
              {#if msg.attachments && msg.attachments.length > 0}
                <div class="attachments">
                  {#each msg.attachments as att}
                    {#if att.mime_type.startsWith("image/") && att.local_path}
                      <img
                        class="attachment-image"
                        src={convertFileSrc(att.local_path)}
                        alt={att.file_name}
                        loading="lazy"
                      />
                    {:else if att.mime_type.startsWith("image/")}
                      <div class="attachment-placeholder">
                        {att.file_name} ({formatSize(att.size)})
                      </div>
                    {:else}
                      <div class="attachment-file">
                        <span class="att-icon">📎</span>
                        <span class="att-name">{att.file_name}</span>
                        <span class="att-size">{formatSize(att.size)}</span>
                      </div>
                    {/if}
                  {/each}
                </div>
              {/if}
              {#if msg.body}
                <p>{msg.body}</p>
              {/if}
              <span class="msg-time">{formatTime(msg.timestamp)}</span>
            </div>
          </div>
        {/each}
      </div>
      {#if pendingFiles.length > 0}
        <div class="pending-files">
          {#each pendingFiles as file, i}
            <div class="pending-file">
              <span>{file.split("/").pop()}</span>
              <button class="remove-file" onclick={() => removePendingFile(i)}>&times;</button>
            </div>
          {/each}
        </div>
      {/if}
      <div class="composer">
        <button class="attach-btn" onclick={handleAttachFile} title="Attach file">
          📎
        </button>
        <input
          type="text"
          placeholder="Message..."
          bind:value={inputText}
          onkeydown={handleKeydown}
        />
        <button class="send-btn" onclick={handleSend}>Send</button>
      </div>

    {:else}
      <div class="empty-state">
        <p>Select a conversation or start a new one</p>
      </div>
    {/if}
  </main>
</div>

<style>
  .new-msg-btn {
    background: var(--accent, #3b82f6);
    color: white;
    border: none;
    border-radius: 50%;
    width: 28px;
    height: 28px;
    font-size: 1.2rem;
    line-height: 1;
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }
  .new-msg-btn:hover {
    background: var(--accent-hover, #2563eb);
  }

  .new-message-form {
    display: flex;
    flex-direction: column;
    gap: 16px;
    padding: 24px;
    max-width: 500px;
  }

  .form-field {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .form-field label {
    font-size: 0.8rem;
    color: var(--text-secondary, #a1a1aa);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .form-field input {
    padding: 10px 12px;
    background: var(--bg-secondary, #16213e);
    border: 1px solid var(--border, #27272a);
    border-radius: 8px;
    color: var(--text-primary, #e4e4e7);
    font-size: 0.95rem;
  }

  .form-field input:focus {
    outline: none;
    border-color: var(--accent, #3b82f6);
  }

  .self-btn {
    align-self: flex-start;
    padding: 4px 10px;
    background: transparent;
    color: var(--accent, #3b82f6);
    border: 1px solid var(--accent, #3b82f6);
    border-radius: 4px;
    font-size: 0.75rem;
    cursor: pointer;
  }

  .self-btn:hover {
    background: var(--accent, #3b82f6);
    color: white;
  }

  .send-error {
    color: #ef4444;
    font-size: 0.85rem;
    margin: 0;
  }

  .form-actions {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
  }

  .primary-btn {
    padding: 8px 20px;
    background: var(--accent, #3b82f6);
    color: white;
    border: none;
    border-radius: 8px;
    font-size: 0.9rem;
    font-weight: 600;
    cursor: pointer;
  }

  .primary-btn:hover {
    background: var(--accent-hover, #2563eb);
  }

  .secondary-btn {
    padding: 8px 20px;
    background: transparent;
    color: var(--text-secondary, #a1a1aa);
    border: 1px solid var(--border, #27272a);
    border-radius: 8px;
    font-size: 0.9rem;
    cursor: pointer;
  }

  .attachments {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-bottom: 4px;
  }

  .attachment-image {
    max-width: 300px;
    max-height: 400px;
    border-radius: 8px;
    object-fit: contain;
  }

  .attachment-placeholder {
    background: rgba(255,255,255,0.05);
    border-radius: 8px;
    padding: 12px;
    font-size: 0.8rem;
    color: var(--text-secondary, #a1a1aa);
  }

  .attachment-file {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 10px;
    background: rgba(255,255,255,0.05);
    border-radius: 6px;
    font-size: 0.85rem;
  }

  .att-name {
    color: var(--accent, #3b82f6);
    flex: 1;
  }

  .att-size {
    color: var(--text-secondary, #a1a1aa);
    font-size: 0.75rem;
  }

  .pending-files {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    padding: 8px 16px;
    border-top: 1px solid var(--border, #27272a);
  }

  .pending-file {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 4px 8px;
    background: var(--bg-secondary, #16213e);
    border-radius: 4px;
    font-size: 0.8rem;
    color: var(--text-primary, #e4e4e7);
  }

  .remove-file {
    background: none;
    border: none;
    color: var(--text-secondary, #a1a1aa);
    cursor: pointer;
    font-size: 1rem;
    padding: 0 2px;
  }

  .attach-btn {
    background: none;
    border: none;
    font-size: 1.2rem;
    cursor: pointer;
    padding: 4px 8px;
    opacity: 0.7;
    transition: opacity 0.15s;
  }

  .attach-btn:hover {
    opacity: 1;
  }
</style>
