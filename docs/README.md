# rebaze Documentation

This directory contains the documentation site for rebaze, built with [Astro Starlight](https://starlight.astro.build/).

## Development

```bash
cd docs

# Install dependencies
bun install

# Start dev server (http://localhost:4321/rebaze)
bun dev

# Build for production
bun build

# Preview production build
bun preview
```

## Structure

```
docs/
├── src/
│   ├── content/docs/      # Documentation pages (MDX)
│   │   ├── getting-started/
│   │   ├── tutorials/
│   │   ├── guides/
│   │   ├── concepts/
│   │   └── reference/
│   ├── assets/            # Logos and images
│   └── styles/            # Custom CSS
├── public/                # Static assets
└── astro.config.mjs       # Starlight configuration
```

## Documentation Organization

The docs follow the [Diataxis](https://diataxis.fr/) framework:

| Section | Purpose | Example |
|---------|---------|---------|
| **Getting Started** | Onboarding new users | Installation, Quick Start |
| **Tutorials** | Learning-oriented walkthroughs | Migrate CMake Project |
| **How-to Guides** | Task-oriented instructions | Use cmake-file-api |
| **Concepts** | Understanding-oriented explanations | How Rebaze Works |
| **Reference** | Information-oriented descriptions | CLI Reference, Config |

## Adding Pages

1. Create an `.mdx` file in the appropriate directory under `src/content/docs/`
2. Add frontmatter with `title` and `description`
3. Add the page to the sidebar in `astro.config.mjs`

Example:

```mdx
---
title: My New Page
description: What this page covers
---

Content goes here...
```

## Deployment

The docs are automatically deployed to GitHub Pages via the `.github/workflows/docs.yml` workflow on pushes to `main`.

Live site: https://albertocavalcante.github.io/rebaze/
