import { defineCollection } from "astro:content";
import { docsLoader } from "@astrojs/starlight/loaders";
import { docsSchema } from "@astrojs/starlight/schema";

// The marketing pages and the docs are one Astro project, so Starlight's
// injected `[...slug]` route shares a namespace with `src/pages/`. Prefixing
// every docs entry id with `docs/` keeps the two apart at a single point of
// configuration: a file at `src/content/docs/concepts/drives.md` serves at
// `/docs/concepts/drives/`, and nothing in the tree has to be nested under a
// second literal `docs/` directory to say so.
const DOCS_PREFIX = "docs";

export const collections = {
  docs: defineCollection({
    loader: docsLoader({
      generateId: ({ entry }) => {
        const slug = entry
          .replace(/\.(md|mdx)$/, "")
          .replace(/(^|\/)index$/, "");
        // Starlight looks up its 404 page by the bare id "404", so that one
        // entry stays unprefixed — it is the site's 404, not the docs' one.
        if (slug === "404") return slug;
        return slug ? `${DOCS_PREFIX}/${slug}` : DOCS_PREFIX;
      },
    }),
    schema: docsSchema(),
  }),
};
