import { Option } from "commander";
import { Sandbox } from "sandkiln";
import { parseEnvVar, parseTag } from "../../format.js";

export const tagOption = () => new Option("--tag <key=value>", "tag to attach (repeatable)").argParser(parseTag).default({});
export const envOption = (help: string) => new Option("--env <key=value>", help).argParser(parseEnvVar).default({});

/** `--egress-mode` is the only required piece for a policy to take
 * effect at all -- `--allow-cidr`/`--deny-cidr` are meaningless without
 * it and are silently ignored (not an error) if given alone, same as
 * omitting an egress policy entirely. */
export function buildEgressOption(opts: { egressMode?: "allow_all" | "deny_all"; allowCidr: string[]; denyCidr: string[] }) {
  if (opts.egressMode === undefined) return undefined;
  return { mode: opts.egressMode, allowCidrs: opts.allowCidr.length > 0 ? opts.allowCidr : undefined, denyCidrs: opts.denyCidr.length > 0 ? opts.denyCidr : undefined };
}

/** Every subcommand below only has a sandbox id, not an instance — this
 * reconstructs one without a round-trip, since every Sandbox method just
 * needs the id plus the same client config already used to reach it. */
export function attachSandbox(id: string, baseUrl: string | undefined, token: string | undefined): Sandbox {
  return Sandbox.attach(id, { baseUrl, authToken: token });
}
