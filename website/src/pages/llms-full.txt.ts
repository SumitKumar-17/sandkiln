import type { APIRoute } from "astro";
import { getCollection } from "astro:content";

// The companion to llms.txt: the same pages, but with each one's full markdown
// body inlined, so a model can read the docs in a single fetch instead of
// following every link. Sections are emitted in the curated reading order
// below (install, then concepts, then tasks, then reference) so the narrative
// survives the flattening; pages within a section fall back to alphabetical.
const SECTION_ORDER = [
  "docs/getting-started/",
  "docs/concepts/",
  "docs/guides/",
  "docs/reference/",
  "docs/architecture/",
];

const rank = (id: string) => {
  const i = SECTION_ORDER.findIndex((p) => id.startsWith(p));
  return i === -1 ? SECTION_ORDER.length : i;
};

export const GET: APIRoute = async ({ site }) => {
  const entries = await getCollection("docs");
  const origin = site ? new URL(import.meta.env.BASE_URL, site).href.replace(/\/$/, "") : "";

  const ordered = entries
    .filter((e) => e.id !== "docs")
    .sort((a, b) => rank(a.id) - rank(b.id) || a.id.localeCompare(b.id));

  const parts: string[] = [
    "# sandkiln — full documentation",
    "",
    "A compute primitive for safely running untrusted or AI-generated code in",
    "hardware-isolated Firecracker microVMs. Self-hosted, MIT licensed.",
    "",
  ];

  for (const entry of ordered) {
    parts.push(
      "---",
      "",
      `# ${entry.data.title}`,
      "",
      `Source: ${origin}/${entry.id}/`,
      "",
    );
    if (entry.data.description) parts.push(entry.data.description, "");
    parts.push((entry.body ?? "").trim(), "");
  }

  return new Response(parts.join("\n"), {
    headers: { "Content-Type": "text/plain; charset=utf-8" },
  });
};
