import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  // Static export — rsynced into /var/www/gray/ beside the existing dl/ tree,
  // so every installer URL keeps being served by nginx untouched.
  output: "export",
  images: { unoptimized: true },
  trailingSlash: true,

  // The sandbox preview reaches the dev server through a tunnel hostname, and
  // Next 16 blocks /_next/hmr from any origin it was not started on — which
  // silently prevents hydration. No effect on the exported build.
  allowedDevOrigins: ["127.0.0.1", "localhost", "*.preview.usehoplite.com"],
};

export default nextConfig;
