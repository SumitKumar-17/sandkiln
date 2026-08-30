import { Sandbox } from "sandkiln";

const PORT = 8080;
const SERVER_SCRIPT_PATH = "/tmp/dev-server.py";

// A stand-in for a real dev server (`npm run dev`, `vite`, `python
// manage.py runserver`, ...) — kept to the Python standard library so
// this example has nothing extra to install inside the sandbox.
const SERVER_SCRIPT = `
import http.server
import socketserver

class Handler(http.server.SimpleHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/html")
        self.end_headers()
        self.wfile.write(b"<h1>hello from inside a sandkiln microVM</h1>")

with socketserver.TCPServer(("0.0.0.0", ${PORT}), Handler) as httpd:
    httpd.serve_forever()
`;

async function main() {
  console.log("Creating sandbox...");
  const sandbox = await Sandbox.create({ tags: { example: "dev-server-preview" } });
  console.log(`Sandbox ${sandbox.id} ready.`);

  try {
    await sandbox.writeFile(SERVER_SCRIPT_PATH, SERVER_SCRIPT);

    // Backgrounded with its own stdout/stderr redirected to a log file —
    // if it inherited the exec call's pipes instead, `sh -c` wouldn't
    // return until the (long-running) server exited, since the
    // backgrounded process would still be holding those pipes open.
    await sandbox.runCommand("sh", [
      "-c",
      `python3 ${SERVER_SCRIPT_PATH} </dev/null >/tmp/dev-server.log 2>&1 & sleep 1`,
    ]);

    const url = sandbox.previewUrl(PORT);
    console.log(`Dev server running inside the sandbox. Preview it at:\n\n  ${url}\n`);
    console.log("Press Ctrl+C to stop the sandbox and exit.");

    await new Promise((resolve) => process.once("SIGINT", resolve));
  } finally {
    await sandbox.stop();
    console.log(`\nSandbox ${sandbox.id} stopped.`);
  }
}

main().catch((err) => {
  console.error(err);
  process.exitCode = 1;
});
