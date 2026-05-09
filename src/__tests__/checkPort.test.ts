import { describe, it, expect, afterEach } from "vitest";
import net from "node:net";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const SCRIPT = path.resolve(__dirname, "../../scripts/check-port.mjs");

let server: net.Server | null = null;

afterEach(() => {
  if (server) {
    server.close();
    server = null;
  }
});

function pickFreePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const s = net.createServer();
    s.unref();
    s.listen(0, "127.0.0.1", () => {
      const addr = s.address();
      if (typeof addr === "object" && addr) {
        const port = addr.port;
        s.close(() => resolve(port));
      } else {
        reject(new Error("no address"));
      }
    });
    s.once("error", reject);
  });
}

function run(port: number): Promise<{ code: number; stderr: string }> {
  return new Promise((resolve, reject) => {
    const proc = spawn("node", [SCRIPT, String(port)], { stdio: ["ignore", "pipe", "pipe"] });
    let stderr = "";
    proc.stderr.on("data", (b) => (stderr += b.toString()));
    proc.on("error", reject);
    proc.on("close", (code) => resolve({ code: code ?? -1, stderr }));
  });
}

describe("scripts/check-port.mjs", () => {
  it("exits 0 when the port is free", async () => {
    const port = await pickFreePort();
    const { code } = await run(port);
    expect(code).toBe(0);
  });

  it("exits 1 with a hint when the port is already bound", async () => {
    server = net.createServer();
    const port: number = await new Promise((resolve, reject) => {
      server!.listen(0, "127.0.0.1", () => {
        const addr = server!.address();
        if (typeof addr === "object" && addr) resolve(addr.port);
        else reject(new Error("no address"));
      });
    });

    const { code, stderr } = await run(port);
    expect(code).toBe(1);
    // Friendly message must include the port and a remediation hint.
    expect(stderr).toContain(String(port));
    expect(stderr.toLowerCase()).toContain("in use");
    expect(stderr).toMatch(/lsof|kill|tauri/i);
  });
});
