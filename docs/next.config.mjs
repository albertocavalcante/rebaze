import { createMDX } from 'fumadocs-mdx/next';

const withMDX = createMDX();

const isGitHubPages = process.env.GITHUB_PAGES === 'true';

/** @type {import('next').NextConfig} */
const config = {
  reactStrictMode: true,

  // Static export for GitHub Pages
  ...(isGitHubPages && {
    output: 'export',
    basePath: '/rebaze',
    images: { unoptimized: true },
    trailingSlash: true,
  }),

  // Rewrites only work in non-static mode
  ...(!isGitHubPages && {
    async rewrites() {
      return [
        {
          source: '/docs/:path*.mdx',
          destination: '/llms.mdx/docs/:path*',
        },
      ];
    },
  }),
};

export default withMDX(config);
