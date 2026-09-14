import type { APIRoute } from "astro";
import { getCollection } from "astro:content";

// The llms.txt convention: a plain-text index of the site at a stable root
// path, so a model ingesting the docs gets the structure and the canonical
// URLs rather than scraping rendered HTML. Generated from the same content
// collection the sidebar is built from, so it can't drift out of sync the way
// a hand-maintained list would.
const SECTIONS: Array<{ prefix: string; label: string }> = [
  { prefix: "docs/getting-started/", label: "Getting Started" },
  { prefix: "docs/concepts/", label: "Core Concepts" },
  { prefix: "docs/guides/", label: "Guides" },
  { prefix: "docs/reference/", label: "Reference" },
  { prefix: "docs/architecture/", label: "Architecture" },
];

export const GET: APIRoute = async ({ site }) => {
  const entries = await getCollection("docs");
  const origin = site ? new URL(import.meta.env.BASE_URL, site).href.replace(/\/$/, "") : "";

  const lines: string[] = [
    "# sandkiln",
    "",
    "> A compute primitive for safely running untrusted or AI-generated code.",
    "> Every sandbox is a real Firecracker microVM with its own kernel, filesystem,",
    "> and network — not a container sharing the host kernel. Self-hosted, MIT licensed.",
    "",
    "Core pieces: a Rust VMM driving Firecracker, a static guest agent reachable over",
    "vsock, an HTTP daemon exposing the lifecycle, plus JS/TS and Python SDKs and the",
    "kiln CLI.",
    "",
  ];

  for (const section of SECTIONS) {
    const inSection = entries
      .filter((e) => e.id.startsWith(section.prefix))
      .sort((a, b) => a.id.localeCompare(b.id));
    if (inSection.length === 0) continue;

    lines.push(`## ${section.label}`, "");
    for (const entry of inSection) {
      const url = `${origin}/${entry.id}/`;
      const desc = entry.data.description ? `: ${entry.data.description}` : "";
      lines.push(`- [${entry.data.title}](${url})${desc}`);
    }
    lines.push("");
  }

  return new Response(lines.join("\n"), {
    headers: { "Content-Type": "text/plain; charset=utf-8" },
  });
};
