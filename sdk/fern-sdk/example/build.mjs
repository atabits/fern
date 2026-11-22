import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";

const exampleDir = path.dirname(fileURLToPath(import.meta.url));
const repoSdkSrc = path.resolve(exampleDir, "../src");
const useRepoSdk = fs.existsSync(path.join(repoSdkSrc, "index.ts"));

const buildOptions = {
  entryPoints: [path.join(exampleDir, "src/index.tsx")],
  outfile: path.join(exampleDir, "dist/index.js"),
  bundle: true,
  format: "esm",
  target: "es2022",
  platform: "neutral",
  mainFields: ["module", "main"],
  external: ["fern", "fern:ui"],
  jsx: "automatic",
  jsxImportSource: "@fern/sdk",
  ...(useRepoSdk
    ? {
        alias: {
          "@fern/sdk": path.join(repoSdkSrc, "index.ts"),
          "@fern/sdk/jsx-runtime": path.join(repoSdkSrc, "jsx-runtime.ts"),
          "@fern/sdk/jsx-dev-runtime": path.join(repoSdkSrc, "jsx-runtime.ts"),
        },
      }
    : {}),
  logLevel: "info",
};

if (process.argv.includes("--watch")) {
  const ctx = await esbuild.context(buildOptions);
  await ctx.watch();
  console.log("Watching plugin sources...");
} else {
  await esbuild.build(buildOptions);
}
