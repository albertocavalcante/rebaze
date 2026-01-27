# Documentation TODO

## GitHub Pages Static Export Limitations

When deploying to GitHub Pages with `output: 'export'`, several features are unavailable.
The build script (`scripts/build-gh-pages.sh`) temporarily disables incompatible routes.

### 1. Server-Side Search API ❌

**Status**: Disabled in static export.

**Issue**: The `/api/search` route uses server-side rendering which doesn't work with static export.

**Current behavior**: Search UI appears but doesn't return results on GitHub Pages.

**Solutions**:
- [ ] **Option A**: Use Orama Cloud (hosted search service)
  - Pros: Full-featured, fast
  - Cons: External dependency, potential cost
  - Docs: https://docs.orama.com/cloud

- [ ] **Option B**: Generate static search index at build time
  - Pros: Self-contained, no external service
  - Cons: Larger bundle size, client-side search
  - Implementation: Use `fumadocs-core/search/client` with pre-built index

- [ ] **Option C**: Use Pagefind (static search)
  - Pros: Battle-tested, used by Starlight
  - Cons: Additional build step
  - Docs: https://pagefind.app

### 2. Per-Page MDX Endpoints ❌

**Status**: Disabled in static export.

**Issue**: The `/llms.mdx/docs/*` routes serve raw markdown per-page but don't work with static export.

**Current behavior**: These endpoints are unavailable on GitHub Pages.

**What works**: `/llms.txt` and `/llms-full.txt` ✅ (contain all docs content)

**Solutions**:
- [ ] **Option A**: Generate static `.mdx` files during build
  - Add a post-build script that copies MDX content to `/out/docs/*.mdx`

- [ ] **Option B**: Use the existing `/llms-full.txt` which contains all content
  - Document this as the recommended approach for LLM consumption

### 3. Dynamic OG Images ✅

**Status**: Working - pre-rendered at build time.

**Issue**: None - Fumadocs generates OG images during static export.

---

## Enhancement Ideas

### Search Improvements
- [ ] Evaluate Orama Cloud vs static search performance
- [ ] Add search analytics to understand usage patterns

### Content
- [ ] Add CMake migration guide with examples
- [ ] Add Gradle migration guide with examples
- [ ] Add troubleshooting section
- [ ] Add FAQ

### Features
- [ ] Add "Edit on GitHub" links to pages
- [ ] Add versioning support for docs
- [ ] Add changelog/release notes page

---

## Build Verification

Test static export locally before pushing:

```bash
cd docs
GITHUB_PAGES=true bun run build
bunx serve out
```

Verify:
- [ ] Home page loads at `/rebaze/`
- [ ] Docs load at `/rebaze/docs/`
- [ ] Navigation works
- [ ] Search works (or gracefully degrades)
- [ ] `/rebaze/llms.txt` is accessible
