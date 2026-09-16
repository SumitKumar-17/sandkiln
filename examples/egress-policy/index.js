import { Sandbox } from "sandkiln";

// A real, reachable IP address to test against -- see README.md for why
// this has to be something *you* supply rather than a hardcoded address.
// A ping is used rather than an HTTP request since it only needs
// something that answers ICMP, not something running a web server.
const targetIp = requireEnv("EGRESS_EXAMPLE_TARGET_IP");

function requireEnv(name) {
  const value = process.env[name];
  if (!value) {
    console.error(`${name} is not set -- this example needs a real, reachable IP address to test against. See README.md.`);
    process.exit(2);
  }
  return value;
}

async function ping(sandbox, label) {
  const result = await sandbox.runCommand("ping", ["-c1", "-W2", targetIp]);
  const reached = result.exitCode === 0;
  console.log(`  ${label}: ${reached ? "reached" : "blocked"} (ping exit code ${result.exitCode})`);
  return reached;
}

async function main() {
  console.log(`Testing egress policy against ${targetIp}.\n`);

  console.log("1. No policy (today's default: unrestricted outbound)");
  const openSandbox = await Sandbox.create({ tags: { example: "egress-policy" } });
  try {
    const reached = await ping(openSandbox, "ping with no policy");
    if (!reached) {
      console.error(`\n${targetIp} isn't reachable at all from a sandbox with no policy -- this example needs a target your network can actually route to. See README.md.`);
      process.exitCode = 2;
      return;
    }
  } finally {
    await openSandbox.stop({ keep: false });
  }

  console.log("\n2. deny_all, no allow_cidrs (blocks everything not explicitly allowed)");
  const deniedSandbox = await Sandbox.create({ tags: { example: "egress-policy" }, egress: { mode: "deny_all" } });
  try {
    const reached = await ping(deniedSandbox, "ping under deny_all");
    console.log(reached ? "  Unexpected: deny_all should have blocked this." : "  As expected: deny_all blocked an unlisted destination.");
  } finally {
    await deniedSandbox.stop({ keep: false });
  }

  console.log(`\n3. deny_all + allow_cidrs: ["${targetIp}/32"] (opens just this one address back up)`);
  const allowedSandbox = await Sandbox.create({
    tags: { example: "egress-policy" },
    egress: { mode: "deny_all", allowCidrs: [`${targetIp}/32`] },
  });
  try {
    const reached = await ping(allowedSandbox, "ping under deny_all + allow_cidrs");
    console.log(reached ? "  As expected: allow_cidrs opened it back up." : "  Unexpected: allow_cidrs should have let this through.");
  } finally {
    await allowedSandbox.stop({ keep: false });
  }

  console.log("\nDone. See ROADMAP.md's \"Firewall and egress policy\" section for the full design.");
}

main().catch((err) => {
  console.error(err);
  process.exitCode = 1;
});
