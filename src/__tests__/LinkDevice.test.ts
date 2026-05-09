import { render, screen, fireEvent } from "@testing-library/svelte";
import { describe, it, expect, vi, beforeEach } from "vitest";

const { mockInvoke, mockListen } = vi.hoisted(() => ({
  mockInvoke: vi.fn().mockResolvedValue(undefined),
  mockListen: vi.fn().mockResolvedValue(() => {}),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: mockInvoke,
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: mockListen,
}));

import LinkDevice from "../lib/components/LinkDevice.svelte";

describe("LinkDevice", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("renders the welcome screen with French copy", () => {
    render(LinkDevice);
    expect(screen.getByText("SignalUI")).toBeInTheDocument();
    // Subtitle and CTA are now French.
    expect(screen.getByText("Un client Signal léger pour le bureau")).toBeInTheDocument();
    expect(screen.getByText("Lier l'appareil")).toBeInTheDocument();
  });

  it("does not leak any English UI strings", () => {
    render(LinkDevice);
    // The previous version had "Link Device", "A lightweight Signal desktop client",
    // "Cancel", "Try Again", etc. Make sure none of them slipped back in.
    const banned = [
      "Link Device",
      "A lightweight Signal desktop client",
      "Connecting to Signal",
      "Linking device",
      "Try Again",
      "Cancel",
    ];
    for (const phrase of banned) {
      expect(screen.queryByText(phrase)).toBeNull();
    }
  });

  it("calls start_provisioning with the localised CTA", async () => {
    render(LinkDevice);
    const btn = screen.getByText("Lier l'appareil");
    expect(btn.tagName).toBe("BUTTON");
    await fireEvent.click(btn);
    expect(mockInvoke).toHaveBeenCalledWith("start_provisioning", {
      deviceName: "SignalUI Desktop",
    });
  });
});
