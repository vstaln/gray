import type { MetadataRoute } from "next";
import { docs } from "@/lib/docs";
import { site } from "@/lib/site";

export const dynamic = "force-static";

export default function sitemap(): MetadataRoute.Sitemap {
  const routes = ["", "/docs", "/credits", "/privacy", "/terms"];
  return [
    ...routes.map((path) => ({
      url: `${site.url}${path}`,
      priority: path === "" ? 1 : 0.7,
    })),
    ...docs.map((d) => ({ url: `${site.url}/docs/${d.slug}`, priority: 0.6 })),
  ];
}
