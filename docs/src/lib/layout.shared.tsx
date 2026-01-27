import type { BaseLayoutProps } from 'fumadocs-ui/layouts/shared';

export function baseOptions(): BaseLayoutProps {
  return {
    nav: {
      title: 'rebaze',
    },
    links: [
      {
        text: 'GitHub',
        url: 'https://github.com/albertocavalcante/rebaze',
        external: true,
      },
    ],
    githubUrl: 'https://github.com/albertocavalcante/rebaze',
  };
}
