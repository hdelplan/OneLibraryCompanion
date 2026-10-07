import { defineConfig } from "vite";
import { readFileSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../../", import.meta.url));

export default defineConfig(({ mode }) => ({
  plugins: [
    {
      name: "olc-third-party-notices",
      generateBundle() {
        const paths = [
          "LICENSE",
          "THIRD_PARTY_NOTICES.md",
          ...readdirSync(`${root}/third-party-licenses`).map(
            (name) => `third-party-licenses/${name}`,
          ),
        ];
        for (const path of paths) {
          this.emitFile({
            type: "asset",
            fileName: `licenses/${path}`,
            source: readFileSync(`${root}/${path}`),
          });
        }
      },
    },
  ],
  define: {
    __OLC_EXPERIMENTS__: JSON.stringify(
      mode === "ipad" || mode === "experiments",
    ),
  },
  build: { target: "safari16", outDir: mode === "ipad" ? "dist-ipad" : "dist" },
  server: { proxy: { "/api": "http://127.0.0.1:8787" } },
}));
