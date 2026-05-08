import { render } from "@testing-library/svelte";
import { describe, it, expect } from "vitest";
import QrCode from "../lib/components/QrCode.svelte";

describe("QrCode", () => {
  it("renders SVG content", () => {
    const testSvg = '<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><rect fill="#fff" width="100" height="100"/></svg>';
    const { container } = render(QrCode, { props: { svg: testSvg } });

    const qrContainer = container.querySelector(".qr-container");
    expect(qrContainer).toBeTruthy();
    expect(qrContainer?.innerHTML).toContain("<svg");
  });

  it("has pointer-events none for security", () => {
    const testSvg = '<svg width="10" height="10"></svg>';
    const { container } = render(QrCode, { props: { svg: testSvg } });

    const qrContainer = container.querySelector(".qr-container");
    expect(qrContainer).toBeTruthy();
  });

  it("has aria label for accessibility", () => {
    const testSvg = '<svg width="10" height="10"></svg>';
    const { container } = render(QrCode, { props: { svg: testSvg } });

    const qrContainer = container.querySelector('[aria-label="QR code for device linking"]');
    expect(qrContainer).toBeTruthy();
  });
});
