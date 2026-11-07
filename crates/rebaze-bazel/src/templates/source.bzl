def _source_repo_impl(repository_ctx):
    """Download and prepare source repository."""
    digest = repository_ctx.attr.sha256
    if len(digest) != 64 or any([char not in "0123456789abcdefABCDEF" for char in digest.elems()]):
        fail("Source archives require a 64-character SHA256 checksum")
    repository_ctx.download_and_extract(
        url = repository_ctx.attr.url,
        sha256 = repository_ctx.attr.sha256,
        stripPrefix = repository_ctx.attr.strip_prefix,
    )

    # Create BUILD.bazel that exports all sources
    repository_ctx.file("BUILD.bazel", '''
filegroup(
    name = "all",
    srcs = glob(["**"]),
    visibility = ["//visibility:public"],
)
''')

_source_repo = repository_rule(
    implementation = _source_repo_impl,
    attrs = {
        "url": attr.string(mandatory = True),
        "sha256": attr.string(mandatory = True),
        "strip_prefix": attr.string(default = ""),
    },
)

def _source_deps_impl(module_ctx):
    """Download all configured source dependencies."""
    for name, config in SOURCES.items():
        if "url" not in config:
            continue
        _source_repo(
            name = name + "_src",
            url = config["url"],
            sha256 = config.get("sha256", ""),
            strip_prefix = config.get("strip_prefix", ""),
        )

source_deps = module_extension(
    implementation = _source_deps_impl,
)
