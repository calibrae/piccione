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

  it("renders welcome screen initially", () => {
    render(LinkDevice);
    expect(screen.getByText("SignalUI")).toBeInTheDocument();
    expect(screen.getByText("Link Device")).toBeInTheDocument();
  });

  it("has a link device button", () => {
    render(LinkDevice);
    const btn = screen.getByText("Link Device");
    expect(btn).toBeInTheDocument();
    expect(btn.tagName).toBe("BUTTON");
  });

  it("calls start_provisioning when link button clicked", async () => {
    render(LinkDevice);
    const btn = screen.getByText("Link Device");
    await fireEvent.click(btn);
    expect(mockInvoke).toHaveBeenCalledWith("start_provisioning", {
      deviceName: "SignalUI Desktop",
    });
  });
});
