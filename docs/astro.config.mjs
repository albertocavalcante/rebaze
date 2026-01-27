// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

// https://astro.build/config
export default defineConfig({
	site: 'https://albertocavalcante.github.io',
	base: '/rebaze',
	integrations: [
		starlight({
			title: 'rebaze',
			description: 'CLI tool for migrating build systems to Bazel',
			expressiveCode: {
				themes: ['catppuccin-mocha', 'catppuccin-latte'],
				styleOverrides: {
					borderRadius: '0.75rem',
					codeFontFamily: "'JetBrains Mono', 'SF Mono', 'Consolas', monospace",
					codeFontSize: '0.875rem',
					codeLineHeight: '1.6',
				},
			},
			logo: {
				light: './src/assets/logo-light.svg',
				dark: './src/assets/logo-dark.svg',
				replacesTitle: true,
			},
			social: [
				{ icon: 'github', label: 'GitHub', href: 'https://github.com/albertocavalcante/rebaze' },
			],
			editLink: {
				baseUrl: 'https://github.com/albertocavalcante/rebaze/edit/main/docs/',
			},
			customCss: ['./src/styles/custom.css'],
			head: [
				{
					tag: 'meta',
					attrs: {
						property: 'og:image',
						content: 'https://albertocavalcante.github.io/rebaze/og.png',
					},
				},
			],
			sidebar: [
				{
					label: 'Getting Started',
					items: [
						{ label: 'Introduction', slug: 'getting-started/introduction' },
						{ label: 'Installation', slug: 'getting-started/installation' },
						{ label: 'Quick Start', slug: 'getting-started/quickstart' },
					],
				},
				{
					label: 'Tutorials',
					items: [
						{ label: 'Migrate CMake Project', slug: 'tutorials/cmake' },
						{ label: 'Migrate Gradle Project', slug: 'tutorials/gradle' },
					],
				},
				{
					label: 'How-to Guides',
					items: [
						{ label: 'Use cmake-file-api', slug: 'guides/cmake-file-api' },
						{ label: 'Build from Source', slug: 'guides/source-builds' },
						{ label: 'Custom Bazel Rules', slug: 'guides/custom-rules' },
						{ label: 'Dependency Mappings', slug: 'guides/dependency-mappings' },
					],
				},
				{
					label: 'Concepts',
					items: [
						{ label: 'How Rebaze Works', slug: 'concepts/how-it-works' },
						{ label: 'Dependency Strategies', slug: 'concepts/dependency-strategies' },
						{ label: 'Generated Files', slug: 'concepts/generated-files' },
					],
				},
				{
					label: 'Reference',
					items: [
						{ label: 'CLI Reference', slug: 'reference/cli' },
						{ label: 'Configuration', slug: 'reference/config' },
					],
				},
			],
			}),
	],
});
