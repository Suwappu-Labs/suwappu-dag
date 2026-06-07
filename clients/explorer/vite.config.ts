import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// suwappu-devnet explorer build config.
//
// Single-page app, hash-routed (no server-side routing needed —
// CloudFront serves index.html for any path; the hash decides
// what to render). The RPC URL is baked in at build time via
// VITE_RPC_URL — default points at the public devnet, override
// for local development.
export default defineConfig({
  plugins: [react()],
  define: {
    __DEFAULT_RPC_URL__: JSON.stringify(
      process.env.VITE_RPC_URL ??
        "https://rpc.devnet.suwappu.globalsettlement.com",
    ),
  },
  build: {
    outDir: "dist",
    sourcemap: false,
    rollupOptions: {
      output: {
        // Inline all assets — keeps the deploy artifact a single
        // directory of small files; CloudFront cache invalidation
        // stays simple.
        manualChunks: undefined,
      },
    },
  },
});
