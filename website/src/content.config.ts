import { defineCollection } from "astro:content";
import { glob } from "astro/loaders";
import { z } from "astro/zod";

// One long-form post per subsystem. `order` is the reading order; the posts
// are written to be read front to back but each stands alone.
const engineering = defineCollection({
  loader: glob({ pattern: "*.md", base: "./src/content/engineering" }),
  schema: z.object({
    title: z.string(),
    summary: z.string(),
    order: z.number(),
    tags: z.array(z.string()).default([]),
    updated: z.coerce.date(),
  }),
});

export const collections = { engineering };
