#!/usr/bin/env node
/**
 * Probe localhost:<port> before vite binds, and bail with a friendly
 * message if it's already in use. We have `strictPort: true` in
 * vite.config.ts, so vite would otherwise crash with a stack trace
 * the moment Cali tries `npm run tauri dev` while another instance
 * is still running.
 *
 * Exit codes:
 *   0  port is free, proceed.
 *   1  port is busy and we printed a hint.
 */
import net from "node:net";

const PORT = Number(process.argv[2] ?? 1420);
const HOST = "127.0.0.1";

function probe(port, host) {
  return new Promise((resolve) => {
    const sock = net.connect({ port, host });
    let settled = false;
    const done = (busy) => {
      if (settled) return;
      settled = true;
      sock.destroy();
      resolve(busy);
    };
    sock.once("connect", () => done(true));
    sock.once("error", () => done(false));
    setTimeout(() => done(false), 250);
  });
}

const busy = await probe(PORT, HOST);
if (busy) {
  const red = (s) => `\x1b[31m${s}\x1b[0m`;
  const dim = (s) => `\x1b[2m${s}\x1b[0m`;
  process.stderr.write(
    [
      "",
      red(`✖ Port ${PORT} (${HOST}) is already in use.`),
      "",
      "  Another vite/tauri dev session is probably still running.",
      "  Find and kill it, then retry:",
      "",
      dim(`    lsof -nP -iTCP:${PORT} -sTCP:LISTEN`),
      dim(`    kill -TERM <pid>   # or pkill -f 'vite|tauri dev'`),
      "",
    ].join("\n") + "\n"
  );
  process.exit(1);
}
