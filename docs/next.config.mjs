import { createMDX } from "fumadocs-mdx/next";
import { resolve } from "path";

const withMDX = createMDX();

/** @type {import('next').NextConfig} */
const config = {
  reactStrictMode: true,
  outputFileTracingRoot: resolve(import.meta.dirname, "./"),
};

export default withMDX(config);
