import { docs } from "@/.source";
import { loader } from "fumadocs-core/source";
import type { VirtualFile } from "fumadocs-core/source";

const mdxSource = docs.toFumadocsSource();

// fumadocs-mdx v11 returns files as a function, fumadocs-core v15 expects an array
const files: VirtualFile[] =
  typeof mdxSource.files === "function"
    ? (mdxSource.files as unknown as () => VirtualFile[])()
    : (mdxSource.files as unknown as VirtualFile[]);

export const source = loader({
  baseUrl: "/docs",
  source: { files },
});
