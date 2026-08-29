import { readFileSync } from "node:fs";
import { Sandbox } from "sandkiln";

const RUNNERS = {
  py: { path: "/tmp/playground.py", command: "python3" },
  js: { path: "/tmp/playground.js", command: "node" },
  sh: { path: "/tmp/playground.sh", command: "bash" },
};

function parseArgs(argv) {
  let lang;
  const rest = [];
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === "--lang") {
      lang = argv[++i];
    } else {
      rest.push(argv[i]);
    }
  }
  return { lang, file: rest[0] };
}

function readStdin() {
  return new Promise((resolve, reject) => {
    let data = "";
    process.stdin.setEncoding("utf8");
    process.stdin.on("data", (chunk) => (data += chunk));
    process.stdin.on("end", () => resolve(data));
    process.stdin.on("error", reject);
  });
}

async function main() {
  const { lang: explicitLang, file } = parseArgs(process.argv.slice(2));
  const code = file ? readFileSync(file, "utf8") : await readStdin();
  const lang = explicitLang ?? file?.split(".").pop() ?? "py";
  const runner = RUNNERS[lang];
  if (!runner) {
    console.error(`Unsupported language "${lang}" — supported: ${Object.keys(RUNNERS).join(", ")}`);
    process.exitCode = 1;
    return;
  }

  console.log("Creating sandbox...");
  const sandbox = await Sandbox.create({ tags: { example: "code-playground" } });
  console.log(`Sandbox ${sandbox.id} ready.`);

  try {
    await sandbox.writeFile(runner.path, code);
    const result = await sandbox.runCommand(runner.command, [runner.path]);

    console.log("--- stdout ---");
    console.log(result.stdout);
    console.log("--- stderr ---");
    console.log(result.stderr);
    console.log(`--- exit code: ${result.exitCode} ---`);
    process.exitCode = result.exitCode;
  } finally {
    await sandbox.stop();
    console.log(`Sandbox ${sandbox.id} stopped.`);
  }
}

main().catch((err) => {
  console.error(err);
  process.exitCode = 1;
});
