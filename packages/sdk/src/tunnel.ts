import type { ClientContext } from "./client.js";
import { request } from "./http.js";

/** Binary WebSocket frame format shared with the daemon's
 * `routes_tunnel.rs` (`encode_frame`/`decode_frame` there) and the Python
 * SDK's own tunnel modules — change one, change all three. One byte op,
 * one byte conn_id length, that many bytes of conn_id, then payload. */
const OP_OPEN = 0;
const OP_DATA = 1;
const OP_CLOSE = 2;

function encodeFrame(op: number, connId: string, payload: Uint8Array): Uint8Array {
  const connIdBytes = new TextEncoder().encode(connId);
  const frame = new Uint8Array(2 + connIdBytes.length + payload.length);
  frame[0] = op;
  frame[1] = connIdBytes.length;
  frame.set(connIdBytes, 2);
  frame.set(payload, 2 + connIdBytes.length);
  return frame;
}

interface DecodedFrame {
  op: number;
  connId: string;
  payload: Uint8Array;
}

function decodeFrame(data: Uint8Array): DecodedFrame | undefined {
  if (data.length < 2) return undefined;
  const op = data[0]!;
  const len = data[1]!;
  if (data.length < 2 + len) return undefined;
  const connId = new TextDecoder().decode(data.slice(2, 2 + len));
  const payload = data.slice(2 + len);
  return { op, connId, payload };
}

export interface TunnelOptions {
  /** Port on the machine running this SDK that the tunnel forwards to. */
  localPort: number;
  /** Defaults to `127.0.0.1` — the real service is almost always local. */
  localHost?: string;
}

export interface TunnelHandle {
  tunnelId: string;
  /** Closes the WebSocket, destroys every open forwarded connection, and
   * tells the guest agent to stop listening. Safe to call more than
   * once. */
  close(): Promise<void>;
}

interface CreateTunnelResponseBody {
  tunnel_id: string;
  guest_port: number;
}

/** Opens a local tunnel: code running inside the sandbox connects to
 * `guestPort` and reaches whatever's listening on `options.localPort` on
 * *this* machine — the reverse of `previewUrl()`, which exposes a port
 * inside the sandbox outward. Node-only: forwarding to a real local TCP
 * socket needs `node:net`, which doesn't exist in a browser, unlike
 * `pty()`/`attachLogs()` which only need a WebSocket and work in either
 * runtime. Dynamically imported rather than a top-level `import` so this
 * module still loads (just can't be called) in a bundler targeting a
 * browser. */
export async function openTunnel(client: ClientContext, sandboxId: string, guestPort: number, options: TunnelOptions): Promise<TunnelHandle> {
  if (typeof WebSocket === "undefined") {
    throw new Error(
      "Sandbox.tunnel() requires a runtime with a global WebSocket implementation (Node.js >= 22) — this runtime doesn't have one.",
    );
  }
  let net: typeof import("node:net");
  try {
    net = await import("node:net");
  } catch {
    throw new Error("Sandbox.tunnel() requires Node.js (needs node:net to forward to a real local socket) — this runtime doesn't have it.");
  }

  const body = await request<CreateTunnelResponseBody>({
    ...client,
    method: "POST",
    path: `/sandboxes/${encodeURIComponent(sandboxId)}/tunnel`,
    body: { guest_port: guestPort },
  });
  const tunnelId = body.tunnel_id;
  const localHost = options.localHost ?? "127.0.0.1";

  const wsBaseUrl = client.baseUrl.replace(/^http/, "ws");
  const url = new URL(`${wsBaseUrl}/sandboxes/${encodeURIComponent(sandboxId)}/tunnel/${encodeURIComponent(tunnelId)}/ws`);
  // Same reason as `pty()`/`attachLogs()`: no WebSocket constructor, in
  // any runtime, can set a custom `Authorization` header.
  if (client.authToken !== undefined) {
    url.searchParams.set("token", client.authToken);
  }

  const ws = new WebSocket(url);
  ws.binaryType = "arraybuffer";
  const sockets = new Map<string, InstanceType<typeof net.Socket>>();

  const closeAllSockets = () => {
    for (const socket of sockets.values()) socket.destroy();
    sockets.clear();
  };

  await new Promise<void>((resolve, reject) => {
    ws.addEventListener("open", () => resolve(), { once: true });
    ws.addEventListener("error", () => reject(new Error(`tunnel ${tunnelId}: WebSocket failed to connect`)), { once: true });
  });

  ws.addEventListener("message", (event) => {
    const frame = decodeFrame(new Uint8Array(event.data as ArrayBuffer));
    if (!frame) return;
    const { op, connId, payload } = frame;

    if (op === OP_OPEN) {
      const socket = net.createConnection({ host: localHost, port: options.localPort });
      sockets.set(connId, socket);
      socket.on("data", (chunk: Buffer) => {
        if (ws.readyState === WebSocket.OPEN) ws.send(encodeFrame(OP_DATA, connId, chunk));
      });
      const onEnd = () => {
        if (sockets.delete(connId) && ws.readyState === WebSocket.OPEN) {
          ws.send(encodeFrame(OP_CLOSE, connId, new Uint8Array(0)));
        }
      };
      socket.on("close", onEnd);
      socket.on("error", onEnd);
    } else if (op === OP_DATA) {
      sockets.get(connId)?.write(payload);
    } else if (op === OP_CLOSE) {
      sockets.get(connId)?.destroy();
      sockets.delete(connId);
    }
  });

  ws.addEventListener("close", closeAllSockets, { once: true });

  let closed = false;
  return {
    tunnelId,
    async close() {
      if (closed) return;
      closed = true;
      ws.close();
      closeAllSockets();
      await request({ ...client, method: "DELETE", path: `/sandboxes/${encodeURIComponent(sandboxId)}/tunnel/${encodeURIComponent(tunnelId)}` }).catch(
        () => {
          // Best-effort -- the daemon-side tunnel is torn down whenever
          // it next notices the guest/socket is gone either way.
        },
      );
    },
  };
}
