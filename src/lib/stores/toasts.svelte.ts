// Hand-rolled Svelte 5 toast store. Zero deps, runes-based.
// Toasts can be informational, errors, or carry a retry callback.

export type ToastKind = "info" | "error" | "success";

export interface Toast {
  id: number;
  message: string;
  kind: ToastKind;
  /** If present, a "Réessayer" button is rendered; on click, the toast is dismissed
   * and this callback is invoked. */
  retry?: () => void | Promise<void>;
  /** Auto-dismiss timeout (ms). 0 disables auto-dismiss. Defaults to 6000 for errors,
   * 4000 otherwise. */
  ttl?: number;
}

export interface ToastInput {
  message: string;
  kind?: ToastKind;
  retry?: () => void | Promise<void>;
  ttl?: number;
}

export function createToastStore() {
  let toasts = $state<Toast[]>([]);
  let nextId = 1;
  // Track timers so we can clear them on manual dismiss.
  const timers = new Map<number, ReturnType<typeof setTimeout>>();

  function push(input: ToastInput): number {
    const id = nextId++;
    const kind = input.kind ?? "info";
    const ttl = input.ttl ?? (kind === "error" ? 6000 : 4000);
    const toast: Toast = {
      id,
      message: input.message,
      kind,
      retry: input.retry,
      ttl,
    };
    toasts = [...toasts, toast];
    if (ttl > 0) {
      const handle = setTimeout(() => dismiss(id), ttl);
      timers.set(id, handle);
    }
    return id;
  }

  function dismiss(id: number) {
    const handle = timers.get(id);
    if (handle) {
      clearTimeout(handle);
      timers.delete(id);
    }
    toasts = toasts.filter((t) => t.id !== id);
  }

  function clear() {
    for (const handle of timers.values()) clearTimeout(handle);
    timers.clear();
    toasts = [];
  }

  /** Convenience: error toast with optional retry. */
  function error(message: string, retry?: () => void | Promise<void>): number {
    return push({ message, kind: "error", retry });
  }

  function info(message: string): number {
    return push({ message, kind: "info" });
  }

  function success(message: string): number {
    return push({ message, kind: "success" });
  }

  return {
    get list(): Toast[] {
      return toasts;
    },
    push,
    dismiss,
    clear,
    error,
    info,
    success,
  };
}

export const toastStore = createToastStore();
